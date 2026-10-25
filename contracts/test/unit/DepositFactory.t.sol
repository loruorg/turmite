// SPDX-License-Identifier: MIT
pragma solidity 0.8.38;

import {DeployHelper} from "../utils/Deploy.sol";
import {DepositFactory} from "../../src/DepositFactory.sol";
import {DepositWallet} from "../../src/DepositWallet.sol";
import {
    MockERC20,
    NoReturnToken,
    BlocklistToken,
    FalseToken,
    FeeToken,
    ReturnBombToken,
    ReentrantToken,
    ExpensiveReceiver,
    RevertingReceiver
} from "../mocks/Mocks.sol";

contract DepositFactoryTest is DeployHelper {
    event Deployed(bytes32 indexed salt, address indexed wallet);
    event Flushed(address indexed wallet, address indexed token, uint256 amount);
    event FlushFailed(address indexed wallet, address indexed token);

    address internal constant NATIVE = address(0);

    address internal cold = makeAddr("cold");
    DepositFactory internal factory;
    MockERC20 internal usdc;
    NoReturnToken internal usdt;

    function setUp() public {
        factory = deployFactory(cold);
        usdc = new MockERC20();
        usdt = new NoReturnToken();
    }

    function salt(uint256 index) internal pure returns (bytes32) {
        return bytes32(index);
    }

    // Construction

    function test_Constructor() public view {
        assertEq(factory.coldWallet(), cold);
        assertEq(factory.implementation(), vm.computeCreateAddress(address(factory), 1));
        DepositWallet impl = DepositWallet(payable(factory.implementation()));
        assertEq(impl.factory(), address(factory));
        assertEq(impl.coldWallet(), cold);
    }

    function test_RevertWhen_ColdWalletIsZero() public {
        vm.expectRevert(DepositFactory.ZeroColdWallet.selector);
        new DepositFactory(address(0));
    }

    function test_FactoryAddressIsTheSameForTheSameColdWallet() public {
        // A plain CREATE gives another address; the CREATE2 deployer gives the same one on every chain.
        assertEq(address(factory), factoryAddress(cold));
        assertTrue(factoryAddress(cold) != factoryAddress(makeAddr("other")));
    }

    function test_FactoryRejectsNativeCoin() public {
        vm.deal(address(this), 1 ether);
        (bool ok,) = address(factory).call{value: 1 ether}("");
        assertFalse(ok);
    }

    // Addresses

    function test_PredictMatchesCreate2Formula() public view {
        bytes32 hash = keccak256(cloneInitCode(factory.implementation()));
        for (uint256 i; i < 5; ++i) {
            assertEq(factory.predict(salt(i)), vm.computeCreate2Address(salt(i), hash, address(factory)));
        }
    }

    function test_CloneCodeIsEip1167() public {
        bytes32 s = salt(1);
        address w = factory.predict(s);
        assertEq(w.code.length, 0);
        factory.flush(one(s), new address[](0));
        assertEq(w.code, cloneRuntime(factory.implementation()));
        assertEq(DepositWallet(payable(w)).coldWallet(), cold);
        assertEq(DepositWallet(payable(w)).factory(), address(factory));
    }

    // Native coin

    function test_NativeBeforeDeployment() public {
        bytes32 s = salt(7);
        address w = factory.predict(s);
        vm.deal(w, 1.5 ether);

        vm.expectEmit(address(factory));
        emit Deployed(s, w);
        vm.expectEmit(address(factory));
        emit Flushed(w, NATIVE, 1.5 ether);
        factory.flush(one(s), list(NATIVE));

        assertEq(w.balance, 0);
        assertEq(cold.balance, 1.5 ether);
    }

    function test_NativeAfterDeployment() public {
        bytes32 s = salt(8);
        address w = factory.predict(s);
        factory.flush(one(s), list(NATIVE));
        assertGt(w.code.length, 0);

        // A plain transfer from an account reaches receive() through the clone.
        address payer = makeAddr("payer");
        vm.deal(payer, 2 ether);
        vm.prank(payer);
        (bool ok,) = w.call{value: 2 ether}("");
        assertTrue(ok);

        vm.recordLogs();
        factory.flush(one(s), list(NATIVE));
        assertEq(vm.getRecordedLogs().length, 1, "only Flushed, no Deployed");
        assertEq(cold.balance, 2 ether);
        assertEq(w.balance, 0);
    }

    function test_NativeNotListedStays() public {
        bytes32 s = salt(9);
        address w = factory.predict(s);
        vm.deal(w, 1 ether);
        usdc.mint(w, 5e6);
        factory.flush(one(s), list(address(usdc)));
        assertEq(w.balance, 1 ether);
        assertEq(usdc.balanceOf(cold), 5e6);
    }

    // Tokens

    function test_StandardToken() public {
        bytes32 s = salt(10);
        address w = factory.predict(s);
        usdc.mint(w, 123e6);

        vm.expectEmit(address(factory));
        emit Deployed(s, w);
        vm.expectEmit(address(factory));
        emit Flushed(w, address(usdc), 123e6);
        factory.flush(one(s), list(address(usdc)));

        assertEq(usdc.balanceOf(w), 0);
        assertEq(usdc.balanceOf(cold), 123e6);
    }

    function test_NoReturnToken() public {
        bytes32 s = salt(11);
        address w = factory.predict(s);
        usdt.mint(w, 77e6);

        vm.expectEmit(address(factory));
        emit Flushed(w, address(usdt), 77e6);
        factory.flush(one(s), list(address(usdt)));

        assertEq(usdt.balanceOf(w), 0);
        assertEq(usdt.balanceOf(cold), 77e6);
    }

    function test_BlockedTokenFailsOthersFlushed() public {
        BlocklistToken bad = new BlocklistToken();
        bytes32 s = salt(12);
        address w = factory.predict(s);
        bad.mint(w, 50);
        bad.setBlocked(w, true);
        usdc.mint(w, 60);
        vm.deal(w, 70);

        vm.expectEmit(address(factory));
        emit FlushFailed(w, address(bad));
        vm.expectEmit(address(factory));
        emit Flushed(w, address(usdc), 60);
        vm.expectEmit(address(factory));
        emit Flushed(w, NATIVE, 70);
        factory.flush(one(s), list(address(bad), address(usdc), NATIVE));

        assertEq(bad.balanceOf(w), 50);
        assertEq(usdc.balanceOf(cold), 60);
        assertEq(cold.balance, 70);

        // After the block ends, a new flush sends the rest.
        bad.setBlocked(w, false);
        factory.flush(one(s), list(address(bad)));
        assertEq(bad.balanceOf(cold), 50);
    }

    function test_ColdWalletBlockedFails() public {
        BlocklistToken bad = new BlocklistToken();
        bytes32 s = salt(13);
        address w = factory.predict(s);
        bad.mint(w, 50);
        bad.setBlocked(cold, true);
        vm.expectEmit(address(factory));
        emit FlushFailed(w, address(bad));
        factory.flush(one(s), list(address(bad)));
        assertEq(bad.balanceOf(w), 50);
    }

    function test_FalseReturnFails() public {
        FalseToken bad = new FalseToken();
        bytes32 s = salt(14);
        address w = factory.predict(s);
        bad.mint(w, 50);
        usdc.mint(w, 1);
        vm.expectEmit(address(factory));
        emit FlushFailed(w, address(bad));
        vm.expectEmit(address(factory));
        emit Flushed(w, address(usdc), 1);
        factory.flush(one(s), list(address(bad), address(usdc)));
        assertEq(bad.balanceOf(w), 50);
    }

    function test_FeeOnTransferToken() public {
        FeeToken fee = new FeeToken();
        bytes32 s = salt(15);
        address w = factory.predict(s);
        fee.mint(w, 1000);
        // The event has the amount that left the wallet. The cold wallet gets it minus the token fee.
        vm.expectEmit(address(factory));
        emit Flushed(w, address(fee), 1000);
        factory.flush(one(s), list(address(fee)));
        assertEq(fee.balanceOf(w), 0);
        assertEq(fee.balanceOf(cold), 990);
    }

    function test_AccountWithoutCodeFails() public {
        bytes32 s = salt(16);
        address w = factory.predict(s);
        address eoa = makeAddr("eoa");
        vm.expectEmit(address(factory));
        emit FlushFailed(w, eoa);
        factory.flush(one(s), list(eoa));
    }

    function test_NonTokenContractsFail() public {
        bytes32 s = salt(17);
        address w = factory.predict(s);
        factory.flush(one(s), new address[](0));
        address[] memory bogus = list(address(factory), factory.implementation(), w);
        vm.recordLogs();
        factory.flush(one(s), bogus);
        assertEq(vm.getRecordedLogs().length, 3, "three FlushFailed");
    }

    function test_ReturnBombCostsLittle() public {
        ReturnBombToken bomb = new ReturnBombToken();
        bytes32 s = salt(18);
        address w = factory.predict(s);
        bomb.mint(w, 5);
        factory.flush(one(s), new address[](0));
        uint256 g = gasleft();
        factory.flush(one(s), list(address(bomb)));
        g -= gasleft();
        assertEq(bomb.balanceOf(cold), 5);
        // The token pays for its own 64 KiB of memory. The wallet copies 32 bytes only.
        assertLt(g, 120_000);
    }

    function test_ZeroBalancesEmitOnlyDeployed() public {
        bytes32 s = salt(19);
        vm.recordLogs();
        factory.flush(one(s), list(NATIVE, address(usdc), address(usdt)));
        assertEq(vm.getRecordedLogs().length, 1);
    }

    function test_DuplicateTokensAndSalts() public {
        bytes32 s = salt(20);
        address w = factory.predict(s);
        usdc.mint(w, 9);
        bytes32[] memory salts = new bytes32[](2);
        salts[0] = s;
        salts[1] = s;
        factory.flush(salts, list(address(usdc), address(usdc)));
        assertEq(usdc.balanceOf(cold), 9);
    }

    // Cold wallet contracts

    function test_ExpensiveColdWallet() public {
        ExpensiveReceiver safe = new ExpensiveReceiver();
        DepositFactory f = deployFactory(address(safe));
        bytes32 s = salt(21);
        address w = f.predict(s);
        vm.deal(w, 3 ether);
        uint256 g = gasleft();
        f.flush(one(s), list(NATIVE));
        g -= gasleft();
        assertEq(address(safe).balance, 3 ether);
        assertGt(g, 800_000, "receive used more than the 2300 stipend");
    }

    function test_RevertingColdWalletNativeFailsTokensFlushed() public {
        RevertingReceiver r = new RevertingReceiver();
        DepositFactory f = deployFactory(address(r));
        bytes32 s = salt(22);
        address w = f.predict(s);
        vm.deal(w, 1 ether);
        usdc.mint(w, 4);
        vm.expectEmit(address(f));
        emit FlushFailed(w, NATIVE);
        vm.expectEmit(address(f));
        emit Flushed(w, address(usdc), 4);
        f.flush(one(s), list(NATIVE, address(usdc)));
        assertEq(w.balance, 1 ether);
        assertEq(usdc.balanceOf(address(r)), 4);
    }

    // Access

    function testFuzz_AnyoneCanFlushOnlyToCold(address caller, uint96 amount) public {
        bytes32 s = salt(23);
        address w = factory.predict(s);
        vm.assume(caller != cold && caller != w);
        vm.deal(w, amount);
        usdc.mint(w, amount);
        uint256 callerBalance = caller.balance;
        vm.prank(caller);
        factory.flush(one(s), list(NATIVE, address(usdc)));
        assertEq(cold.balance, amount);
        assertEq(usdc.balanceOf(cold), amount);
        assertEq(caller.balance, callerBalance);
        assertEq(usdc.balanceOf(caller), 0);
    }

    function testFuzz_RevertWhen_WalletCalledDirectly(address caller) public {
        vm.assume(caller != address(factory));
        bytes32 s = salt(24);
        address w = factory.predict(s);
        factory.flush(one(s), new address[](0));
        address impl = factory.implementation();

        vm.prank(caller);
        vm.expectRevert(DepositWallet.NotFactory.selector);
        DepositWallet(payable(w)).flush(list(NATIVE));

        vm.prank(caller);
        vm.expectRevert(DepositWallet.NotFactory.selector);
        DepositWallet(payable(impl)).flush(list(NATIVE));
    }

    function testFuzz_RandomCalldataCannotMoveFunds(address caller, bytes calldata data) public {
        vm.assume(caller != address(factory));
        bytes32 s = salt(25);
        address w = factory.predict(s);
        factory.flush(one(s), new address[](0));
        vm.deal(w, 1 ether);
        usdc.mint(w, 100);
        vm.prank(caller);
        (bool ok,) = w.call(data);
        ok;
        assertEq(w.balance, 1 ether);
        assertEq(usdc.balanceOf(w), 100);
    }

    // Repeat flush and batches

    function test_ReflushDeployedWallet() public {
        bytes32 s = salt(26);
        address w = factory.predict(s);
        for (uint256 round = 1; round <= 3; ++round) {
            vm.deal(w, round);
            usdc.mint(w, round);
            factory.flush(one(s), list(NATIVE, address(usdc)));
            assertEq(w.balance, 0);
            assertEq(usdc.balanceOf(w), 0);
        }
        assertEq(cold.balance, 6);
        assertEq(usdc.balanceOf(cold), 6);
    }

    function test_ManySaltsInOneCall() public {
        uint256 n = 100;
        bytes32[] memory salts = new bytes32[](n);
        for (uint256 i; i < n; ++i) {
            salts[i] = salt(1000 + i);
            address w = factory.predict(salts[i]);
            vm.deal(w, i + 1);
            usdc.mint(w, i + 1);
            usdt.mint(w, i + 1);
        }
        factory.flush(salts, list(NATIVE, address(usdc), address(usdt)));
        uint256 total = n * (n + 1) / 2;
        assertEq(cold.balance, total);
        assertEq(usdc.balanceOf(cold), total);
        assertEq(usdt.balanceOf(cold), total);
    }

    function test_RevertWhen_MoreThan256Tokens() public {
        vm.expectRevert(DepositFactory.TooManyTokens.selector);
        factory.flush(one(salt(27)), new address[](257));
    }

    function test_Token256FailureBit() public {
        bytes32 s = salt(28);
        address w = factory.predict(s);
        address[] memory tokens = new address[](256);
        tokens[255] = makeAddr("eoa");
        vm.deal(w, 1);
        vm.expectEmit(address(factory));
        emit Flushed(w, NATIVE, 1);
        vm.expectEmit(address(factory));
        emit FlushFailed(w, tokens[255]);
        factory.flush(one(s), tokens);
    }

    // Reentrancy

    function test_ReentrantTokenCannotRedirect() public {
        ReentrantToken re = new ReentrantToken();
        bytes32 s1 = salt(29);
        bytes32 s2 = salt(30);
        address w1 = factory.predict(s1);
        address w2 = factory.predict(s2);
        re.mint(w1, 10);
        re.mint(w2, 20);
        vm.deal(w1, 1);
        vm.deal(w2, 2);
        usdc.mint(w1, 3);

        bytes32[] memory both = new bytes32[](2);
        both[0] = s2;
        both[1] = s1;
        re.arm(factory, both, list(NATIVE, address(re), address(usdc)));
        factory.flush(one(s1), list(address(re), NATIVE, address(usdc)));

        assertEq(re.balanceOf(cold), 30);
        assertEq(cold.balance, 3);
        assertEq(usdc.balanceOf(cold), 3);
        assertEq(w1.balance + w2.balance + re.balanceOf(w1) + re.balanceOf(w2) + usdc.balanceOf(w1), 0);
    }

    // Fuzz

    function testFuzz_FlushRandomSaltAndAmounts(bytes32 s, uint128 nat, uint128 a, uint128 b) public {
        address w = factory.predict(s);
        assertEq(w, vm.computeCreate2Address(s, keccak256(cloneInitCode(factory.implementation())), address(factory)));
        vm.deal(w, nat);
        usdc.mint(w, a);
        usdt.mint(w, b);
        factory.flush(one(s), list(NATIVE, address(usdc), address(usdt)));
        assertEq(cold.balance, nat);
        assertEq(usdc.balanceOf(cold), a);
        assertEq(usdt.balanceOf(cold), b);
        assertEq(w.balance + usdc.balanceOf(w) + usdt.balanceOf(w), 0);
        assertGt(w.code.length, 0);
    }

    function testFuzz_ManyRandomSalts(uint256 seed, uint8 count) public {
        uint256 n = bound(count, 1, 40);
        bytes32[] memory salts = new bytes32[](n);
        uint256 total;
        for (uint256 i; i < n; ++i) {
            salts[i] = keccak256(abi.encode(seed, i));
            uint256 amount = uint256(keccak256(abi.encode(salts[i]))) % 1e30;
            total += amount;
            address w = factory.predict(salts[i]);
            vm.deal(w, amount);
            usdc.mint(w, amount);
        }
        factory.flush(salts, list(NATIVE, address(usdc)));
        assertEq(cold.balance, total);
        assertEq(usdc.balanceOf(cold), total);
    }
}
