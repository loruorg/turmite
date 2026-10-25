// SPDX-License-Identifier: MIT
pragma solidity 0.8.38;

import {DeployHelper} from "../utils/Deploy.sol";
import {DepositFactory} from "../../src/DepositFactory.sol";

interface IERC20 {
    function balanceOf(address) external view returns (uint256);
}

interface IUsdc {
    function blacklister() external view returns (address);
    function blacklist(address) external;
}

interface IUsdt {
    function owner() external view returns (address);
    function addBlackList(address) external;
}

/// Real tokens on mainnet forks. Run with FOUNDRY_PROFILE=fork. Needs network access.
contract ForkTest is DeployHelper {
    event Flushed(address indexed wallet, address indexed token, uint256 amount);
    event FlushFailed(address indexed wallet, address indexed token);

    address internal constant ETH_USDC = 0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48;
    address internal constant ETH_USDT = 0xdAC17F958D2ee523a2206206994597C13D831ec7;
    address internal constant BASE_USDC = 0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913;
    address internal constant POLYGON_USDC = 0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359;
    address internal constant POLYGON_USDT = 0xc2132D05D31c914a87C6611C10748AEb04B58e8F;
    address internal constant BSC_USDC = 0x8AC76a51cc950d9822D68b83fE1Ad97B32Cd580d;
    address internal constant BSC_USDT = 0x55d398326f99059fF775485246999027B3197955;

    address internal cold = makeAddr("cold");

    function _setUp(string memory chain) internal returns (DepositFactory f, address w) {
        vm.createSelectFork(chain);
        assertGt(CREATE2_DEPLOYER.code.length, 0, "no CREATE2 deployer");
        f = deployFactory(cold);
        w = f.predict(bytes32(uint256(1)));
    }

    function _flushAll(string memory chain, address t1, address t2) internal {
        (DepositFactory f, address w) = _setUp(chain);
        vm.deal(w, 1 ether);
        deal(t1, w, 1000e6);
        if (t2 != address(0)) deal(t2, w, 2000e6);
        f.flush(one(bytes32(uint256(1))), list(address(0), t1, t2));
        assertEq(cold.balance, 1 ether);
        assertEq(IERC20(t1).balanceOf(cold), 1000e6);
        assertEq(IERC20(t1).balanceOf(w), 0);
        if (t2 != address(0)) {
            assertEq(IERC20(t2).balanceOf(cold), 2000e6);
            assertEq(IERC20(t2).balanceOf(w), 0);
        }
    }

    function test_Ethereum() public {
        _flushAll("ethereum", ETH_USDC, ETH_USDT);
    }

    function test_Base() public {
        _flushAll("base", BASE_USDC, address(0));
    }

    function test_Polygon() public {
        _flushAll("polygon", POLYGON_USDC, POLYGON_USDT);
    }

    function test_Bsc() public {
        _flushAll("bsc", BSC_USDC, BSC_USDT);
    }

    function test_EthereumUsdcBlacklistedWallet() public {
        (DepositFactory f, address w) = _setUp("ethereum");
        deal(ETH_USDC, w, 5e6);
        deal(ETH_USDT, w, 6e6);
        vm.prank(IUsdc(ETH_USDC).blacklister());
        IUsdc(ETH_USDC).blacklist(w);
        vm.expectEmit(address(f));
        emit FlushFailed(w, ETH_USDC);
        vm.expectEmit(address(f));
        emit Flushed(w, ETH_USDT, 6e6);
        f.flush(one(bytes32(uint256(1))), list(ETH_USDC, ETH_USDT));
        assertEq(IERC20(ETH_USDT).balanceOf(cold), 6e6);
    }

    function test_EthereumUsdtBlacklistedWallet() public {
        (DepositFactory f, address w) = _setUp("ethereum");
        deal(ETH_USDT, w, 6e6);
        vm.prank(IUsdt(ETH_USDT).owner());
        IUsdt(ETH_USDT).addBlackList(w);
        vm.expectEmit(address(f));
        emit FlushFailed(w, ETH_USDT);
        f.flush(one(bytes32(uint256(1))), list(ETH_USDT));
        assertEq(IERC20(ETH_USDT).balanceOf(w), 6e6);
    }
}
