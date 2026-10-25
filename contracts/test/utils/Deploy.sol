// SPDX-License-Identifier: MIT
pragma solidity 0.8.38;

import {Test} from "forge-std/Test.sol";
import {DepositFactory} from "../../src/DepositFactory.sol";

/// Shared helpers. The factory is deployed through the CREATE2 deployer at 0x4e59..., as in production.
abstract contract DeployHelper is Test {
    address internal constant CREATE2_DEPLOYER = 0x4e59b44847b379578588920cA78FbF26c0B4956C;
    bytes internal constant CREATE2_DEPLOYER_CODE =
        hex"7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffe03601600081602082378035828234f58015156039578182fd5b8082525050506014600cf3";
    bytes32 internal constant FACTORY_SALT = bytes32(0);

    function factoryInitCode(address cold) internal pure returns (bytes memory) {
        return abi.encodePacked(type(DepositFactory).creationCode, abi.encode(cold));
    }

    function factoryAddress(address cold) internal pure returns (address) {
        return vm.computeCreate2Address(FACTORY_SALT, keccak256(factoryInitCode(cold)), CREATE2_DEPLOYER);
    }

    function deployFactory(address cold) internal returns (DepositFactory) {
        if (CREATE2_DEPLOYER.code.length == 0) vm.etch(CREATE2_DEPLOYER, CREATE2_DEPLOYER_CODE);
        (bool ok, bytes memory ret) = CREATE2_DEPLOYER.call(abi.encodePacked(FACTORY_SALT, factoryInitCode(cold)));
        require(ok, "deploy");
        // The deployer returns the 20-byte address.
        // forge-lint: disable-next-line(unsafe-typecast)
        address f = address(bytes20(ret));
        require(f == factoryAddress(cold), "address");
        return DepositFactory(f);
    }

    function cloneInitCode(address impl) internal pure returns (bytes memory) {
        return
            abi.encodePacked(hex"3d602d80600a3d3981f3363d3d373d3d3d363d73", impl, hex"5af43d82803e903d91602b57fd5bf3");
    }

    function cloneRuntime(address impl) internal pure returns (bytes memory) {
        return abi.encodePacked(hex"363d3d373d3d3d363d73", impl, hex"5af43d82803e903d91602b57fd5bf3");
    }

    function one(bytes32 s) internal pure returns (bytes32[] memory a) {
        a = new bytes32[](1);
        a[0] = s;
    }

    function list(address a0) internal pure returns (address[] memory a) {
        a = new address[](1);
        a[0] = a0;
    }

    function list(address a0, address a1) internal pure returns (address[] memory a) {
        a = new address[](2);
        a[0] = a0;
        a[1] = a1;
    }

    function list(address a0, address a1, address a2) internal pure returns (address[] memory a) {
        a = new address[](3);
        a[0] = a0;
        a[1] = a1;
        a[2] = a2;
    }
}
