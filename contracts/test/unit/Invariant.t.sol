// SPDX-License-Identifier: MIT
pragma solidity 0.8.38;

import {Test} from "forge-std/Test.sol";
import {DeployHelper} from "../utils/Deploy.sol";
import {DepositFactory} from "../../src/DepositFactory.sol";
import {MockERC20, NoReturnToken, BlocklistToken, ReentrantToken} from "../mocks/Mocks.sol";

/// Random funding, flushes, block-list changes, reentrant tokens and raw calls to the clones.
contract Handler is Test {
    uint256 internal constant SALTS = 8;

    DepositFactory public immutable factory;
    address public immutable cold;
    MockERC20 public immutable usdc;
    NoReturnToken public immutable usdt;
    BlocklistToken public immutable blk;
    ReentrantToken public immutable re;

    bytes32[SALTS] public salts;
    address[] internal pool;
    uint256 public ghostNative;
    mapping(address => uint256) public ghostMinted;

    constructor(DepositFactory f, address cold_) {
        factory = f;
        cold = cold_;
        usdc = new MockERC20();
        usdt = new NoReturnToken();
        blk = new BlocklistToken();
        re = new ReentrantToken();
        for (uint256 i; i < SALTS; ++i) {
            salts[i] = keccak256(abi.encode("salt", i));
        }
        pool.push(address(0));
        pool.push(address(usdc));
        pool.push(address(usdt));
        pool.push(address(blk));
        pool.push(address(re));
        pool.push(address(f));
        pool.push(f.implementation());
        pool.push(f.predict(salts[0]));
        pool.push(cold_);
        pool.push(address(0xdead));
    }

    function wallet(uint256 i) public view returns (address) {
        return factory.predict(salts[i]);
    }

    function tokens() external view returns (address[4] memory) {
        return [address(usdc), address(usdt), address(blk), address(re)];
    }

    function fundNative(uint256 saltSeed, uint256 amount) external {
        amount = bound(amount, 0, 1e30);
        vm.deal(address(this), amount);
        (bool ok,) = wallet(saltSeed % SALTS).call{value: amount}("");
        require(ok, "fund");
        ghostNative += amount;
    }

    function fundToken(uint256 saltSeed, uint256 tokenSeed, uint256 amount) external {
        amount = bound(amount, 0, 1e30);
        address w = wallet(saltSeed % SALTS);
        uint256 t = tokenSeed % 4;
        if (t == 0) usdc.mint(w, amount);
        else if (t == 1) usdt.mint(w, amount);
        else if (t == 2) blk.mint(w, amount);
        else re.mint(w, amount);
        ghostMinted[[address(usdc), address(usdt), address(blk), address(re)][t]] += amount;
    }

    function flush(uint256 saltMask, uint256 assetMask, address caller) external {
        vm.prank(_caller(caller));
        factory.flush(_salts(saltMask), _assets(assetMask));
    }

    function setBlocked(uint256 saltSeed, bool blockCold, bool value) external {
        blk.setBlocked(blockCold ? cold : wallet(saltSeed % SALTS), value);
    }

    function armReentrant(uint256 saltMask, uint256 assetMask) external {
        re.arm(factory, _salts(saltMask), _assets(assetMask));
    }

    function rawCall(uint256 saltSeed, address caller, bytes calldata data) external {
        address w = wallet(saltSeed % SALTS);
        vm.prank(_caller(caller));
        (bool ok,) = w.call(data);
        ok;
    }

    function _caller(address c) internal pure returns (address) {
        return c == address(vm) ? address(0xbeef) : c;
    }

    function _salts(uint256 mask) internal view returns (bytes32[] memory out) {
        out = new bytes32[](SALTS);
        uint256 n;
        for (uint256 i; i < SALTS; ++i) {
            if (mask & (1 << i) != 0) out[n++] = salts[i];
        }
        assembly {
            mstore(out, n)
        }
    }

    function _assets(uint256 mask) internal view returns (address[] memory out) {
        out = new address[](pool.length);
        uint256 n;
        for (uint256 i; i < pool.length; ++i) {
            if (mask & (1 << i) != 0) out[n++] = pool[i];
        }
        assembly {
            mstore(out, n)
        }
    }
}

contract InvariantTest is DeployHelper {
    DepositFactory internal factory;
    Handler internal handler;
    address internal cold = makeAddr("cold");

    function setUp() public {
        factory = deployFactory(cold);
        handler = new Handler(factory, cold);
        targetContract(address(handler));
    }

    /// Native coin is either at a deposit address or at the cold wallet.
    function invariant_NativeOnlyToCold() public view {
        uint256 sum = cold.balance;
        for (uint256 i; i < 8; ++i) {
            sum += handler.wallet(i).balance;
        }
        assertEq(sum, handler.ghostNative());
        assertEq(address(factory).balance, 0);
        assertEq(factory.implementation().balance, 0);
        assertEq(address(handler).balance, 0);
    }

    /// Each token is either at a deposit address or at the cold wallet.
    function invariant_TokensOnlyToCold() public view {
        address[4] memory ts = handler.tokens();
        for (uint256 t; t < 4; ++t) {
            MockERC20 token = MockERC20(ts[t]);
            uint256 sum = token.balanceOf(cold);
            for (uint256 i; i < 8; ++i) {
                sum += token.balanceOf(handler.wallet(i));
            }
            assertEq(sum, handler.ghostMinted(ts[t]));
            assertEq(token.totalSupply(), handler.ghostMinted(ts[t]));
        }
    }

    /// The configuration never changes.
    function invariant_ConfigFixed() public view {
        assertEq(factory.coldWallet(), cold);
        assertEq(factory.implementation(), vm.computeCreateAddress(address(factory), 1));
    }
}
