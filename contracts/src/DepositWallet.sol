// SPDX-License-Identifier: MIT
pragma solidity 0.8.38;

/// @title DepositWallet
/// @notice Logic of the deposit addresses. Each deposit address is an EIP-1167 clone of this contract.
/// A clone can only send its full balance of an asset to the cold wallet. Only the factory can start a send.
/// @dev The clones have no storage. The factory and the cold wallet are immutables of this implementation.
contract DepositWallet {
    /// @notice The factory that deployed this implementation.
    address public immutable factory;
    /// @notice The only recipient of all funds.
    address public immutable coldWallet;

    error NotFactory();

    constructor(address coldWallet_) {
        factory = msg.sender;
        coldWallet = coldWallet_;
    }

    receive() external payable {}

    /// @notice Sends the full balance of each asset to the cold wallet. Address zero is the native coin.
    /// @dev A failed asset does not stop the others. The factory limits the list to 256 assets.
    /// @return sent The amount that left this wallet, per asset. Zero for a failed asset.
    /// @return failed Bit i is set when asset i failed.
    function flush(address[] calldata assets) external returns (uint256[] memory sent, uint256 failed) {
        if (msg.sender != factory) revert NotFactory();
        sent = new uint256[](assets.length);
        for (uint256 i; i < assets.length; ++i) {
            (bool ok, uint256 amount) = assets[i] == address(0) ? _sendNative() : _sendToken(assets[i]);
            if (ok) sent[i] = amount;
            // forge-lint: disable-next-line(incorrect-shift)
            else failed |= 1 << i;
        }
    }

    function _sendNative() private returns (bool ok, uint256 amount) {
        amount = address(this).balance;
        if (amount == 0) return (true, 0);
        address to = coldWallet;
        // Forward all gas: the cold wallet can be a contract (for example a Safe). Copy no return data.
        assembly ("memory-safe") {
            ok := call(gas(), to, amount, 0, 0, 0, 0)
        }
    }

    /// @dev Accepts tokens that return true and tokens that return nothing. Copies at most 32 bytes of
    /// return data, so a large return value cannot use all the gas.
    function _sendToken(address token) private returns (bool ok, uint256 amount) {
        address to = coldWallet;
        assembly ("memory-safe") {
            let m := mload(0x40)
            mstore(m, 0x70a08231) // balanceOf(address)
            mstore(add(m, 0x20), address())
            // Yul evaluates the arguments from right to left: staticcall runs before returndatasize.
            if and(gt(returndatasize(), 0x1f), staticcall(gas(), token, add(m, 0x1c), 0x24, m, 0x20)) {
                amount := mload(m)
                ok := 1
                if amount {
                    mstore(m, 0xa9059cbb) // transfer(address,uint256)
                    mstore(add(m, 0x20), to)
                    mstore(add(m, 0x40), amount)
                    ok := call(gas(), token, 0, add(m, 0x1c), 0x44, m, 0x20)
                    // No return data, or exactly true in the first word. A token without code
                    // cannot get here: its balanceOf call returned 32 bytes.
                    ok := and(ok, or(iszero(returndatasize()), and(gt(returndatasize(), 0x1f), eq(mload(m), 1))))
                }
            }
        }
    }
}
