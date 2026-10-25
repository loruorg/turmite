// SPDX-License-Identifier: MIT
pragma solidity 0.8.38;

import {DepositWallet} from "./DepositWallet.sol";

/// @title DepositFactory
/// @notice Makes one deposit address per payment and sends the funds at these addresses to one cold wallet.
/// The cold wallet is fixed at deployment. The contract has no owner, no admin, no upgrade path and no
/// other recipient. Anyone can call flush.
/// @dev A deposit address is the CREATE2 address of an EIP-1167 clone of the implementation. It can receive
/// funds before the clone exists. The salt is a 32-byte value that the server chooses for each payment.
contract DepositFactory {
    /// @notice The only recipient of all funds.
    address public immutable coldWallet;
    /// @notice The DepositWallet implementation. Its address is the CREATE address of this factory at nonce 1.
    address public immutable implementation;

    bytes32 private immutable CLONE_CODE_HASH;

    event Deployed(bytes32 indexed salt, address indexed wallet);
    /// @dev token is address zero for the native coin. amount is the amount that left the wallet.
    event Flushed(address indexed wallet, address indexed token, uint256 amount);
    event FlushFailed(address indexed wallet, address indexed token);

    error ZeroColdWallet();
    error TooManyTokens();
    error DeployFailed();

    constructor(address coldWallet_) {
        if (coldWallet_ == address(0)) revert ZeroColdWallet();
        coldWallet = coldWallet_;
        address impl = address(new DepositWallet(coldWallet_));
        implementation = impl;
        CLONE_CODE_HASH = keccak256(_cloneInitCode(impl));
    }

    /// @notice The deposit address for a salt. The address does not change when the clone is deployed.
    function predict(bytes32 salt) public view returns (address) {
        return
            address(uint160(uint256(keccak256(abi.encodePacked(bytes1(0xff), address(this), salt, CLONE_CODE_HASH)))));
    }

    /// @notice For each salt, deploys the clone if it has no code, then sends the full balance of each token to the
    /// cold wallet. Address zero in tokens is the native coin.
    /// @dev A token that fails emits FlushFailed and does not stop the other tokens or salts.
    function flush(bytes32[] calldata salts, address[] calldata tokens) external {
        if (tokens.length > 256) revert TooManyTokens();
        for (uint256 i; i < salts.length; ++i) {
            bytes32 salt = salts[i];
            address wallet = predict(salt);
            if (wallet.code.length == 0) {
                bytes memory code = _cloneInitCode(implementation);
                address deployed;
                assembly ("memory-safe") {
                    deployed := create2(0, add(code, 0x20), mload(code), salt)
                }
                if (deployed == address(0)) revert DeployFailed();
                emit Deployed(salt, wallet);
            }
            (uint256[] memory sent, uint256 failed) = DepositWallet(payable(wallet)).flush(tokens);
            for (uint256 j; j < tokens.length; ++j) {
                // forge-lint: disable-next-line(incorrect-shift)
                if (failed & (1 << j) != 0) emit FlushFailed(wallet, tokens[j]);
                else if (sent[j] != 0) emit Flushed(wallet, tokens[j], sent[j]);
            }
        }
    }

    /// @dev EIP-1167 minimal proxy init code (55 bytes).
    function _cloneInitCode(address impl) private pure returns (bytes memory) {
        return
            abi.encodePacked(hex"3d602d80600a3d3981f3363d3d373d3d3d363d73", impl, hex"5af43d82803e903d91602b57fd5bf3");
    }
}
