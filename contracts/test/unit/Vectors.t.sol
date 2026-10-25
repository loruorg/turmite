// SPDX-License-Identifier: MIT
pragma solidity 0.8.38;

import {console} from "forge-std/console.sol";
import {DeployHelper} from "../utils/Deploy.sol";
import {DepositFactory} from "../../src/DepositFactory.sol";

/// Fixed addresses for the off-chain code. If this test fails, the bytecode changed:
/// run script/export-min.sh and update the vectors in crates/turmite/src/chains/evm_factory.rs.
contract VectorsTest is DeployHelper {
    address internal constant COLD = 0x00000000000000000000000000000000c01d0001;

    function test_Vectors() public {
        DepositFactory f = deployFactory(COLD);
        address impl = f.implementation();
        bytes32 cloneHash = keccak256(cloneInitCode(impl));
        console.log("cold", COLD);
        console.log("factory", address(f));
        console.log("implementation", impl);
        console.logBytes32(cloneHash);

        assertEq(address(f), 0x3AB39560dF2B6336624F50a135Ad4D3Ad1161892);
        assertEq(impl, 0xb611924EdeD1fc9717921F14B54Ab32E9386dcA3);
        assertEq(cloneHash, 0x0e9e2845fef024dbbc5c120dffb10a750abf3d4f0449050501156c0e5d1056dd);

        uint256[4] memory idx = [uint256(0), 1, 42, type(uint64).max];
        address[4] memory want = [
            0x170b0A3c92AFd6Bb16395e431CA179f43e9001d7,
            0xB74e7c90F860d2a9ccD374D4E6f6fAf0061615f4,
            0x1f50D60B339b180d66eAd7212052579790a2D450,
            0xAeE9Cd4e6AbAb719E6Cc0Fd428c0e29f9Fac01a3
        ];
        for (uint256 i; i < idx.length; ++i) {
            address got = f.predict(bytes32(idx[i]));
            console.log(idx[i], got);
            assertEq(got, want[i]);
        }

        bytes32 full = keccak256("turmite");
        console.logBytes32(full);
        console.log("full salt", f.predict(full));
        assertEq(f.predict(full), 0x1083Aa379CCF763B7d46fA50abdaEE5dD64f9F47);

        // Formula only, with addresses that do not depend on the bytecode.
        address free = vm.computeCreate2Address(
            bytes32(uint256(1)),
            keccak256(cloneInitCode(0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512)),
            0x5FbDB2315678afecb367f032d93F642f64180aa3
        );
        console.log("formula", free);
        assertEq(free, 0x82f890a4981b94E87d37Cb55CFb07a8E16c3937f);
    }
}
