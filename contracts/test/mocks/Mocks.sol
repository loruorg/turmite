// SPDX-License-Identifier: MIT
pragma solidity 0.8.38;

import {DepositFactory} from "../../src/DepositFactory.sol";

/// Standard token: transfer returns true.
contract MockERC20 {
    mapping(address => uint256) public balanceOf;
    uint256 public totalSupply;

    event Transfer(address indexed from, address indexed to, uint256 value);

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
        totalSupply += amount;
        emit Transfer(address(0), to, amount);
    }

    function transfer(address to, uint256 amount) external virtual returns (bool) {
        _move(msg.sender, to, amount);
        return true;
    }

    function _move(address from, address to, uint256 amount) internal {
        require(balanceOf[from] >= amount, "balance");
        balanceOf[from] -= amount;
        balanceOf[to] += amount;
        emit Transfer(from, to, amount);
    }
}

/// Token that returns nothing from transfer, as USDT on Ethereum.
contract NoReturnToken {
    mapping(address => uint256) public balanceOf;
    uint256 public totalSupply;

    function mint(address to, uint256 amount) external {
        balanceOf[to] += amount;
        totalSupply += amount;
    }

    function transfer(address to, uint256 amount) external {
        require(balanceOf[msg.sender] >= amount, "balance");
        balanceOf[msg.sender] -= amount;
        balanceOf[to] += amount;
    }
}

/// Token that reverts when the sender or the recipient is on its block list.
contract BlocklistToken is MockERC20 {
    mapping(address => bool) public blocked;

    function setBlocked(address who, bool value) external {
        blocked[who] = value;
    }

    function transfer(address to, uint256 amount) external override returns (bool) {
        require(!blocked[msg.sender] && !blocked[to], "blocked");
        _move(msg.sender, to, amount);
        return true;
    }
}

/// Token that returns false and moves nothing.
contract FalseToken is MockERC20 {
    function transfer(address, uint256) external pure override returns (bool) {
        return false;
    }
}

/// Token that burns 1% of each transfer.
contract FeeToken is MockERC20 {
    function transfer(address to, uint256 amount) external override returns (bool) {
        uint256 fee = amount / 100;
        _move(msg.sender, to, amount - fee);
        balanceOf[msg.sender] -= fee;
        totalSupply -= fee;
        return true;
    }
}

/// Token that returns 64 KiB of data from transfer. The first word is true.
contract ReturnBombToken is MockERC20 {
    function transfer(address to, uint256 amount) external override returns (bool) {
        _move(msg.sender, to, amount);
        assembly {
            mstore(0, 1)
            return(0, 0x10000)
        }
    }
}

/// Token that calls the factory again from transfer.
contract ReentrantToken is MockERC20 {
    DepositFactory public factory;
    bytes32[] internal salts;
    address[] internal tokens;

    function arm(DepositFactory f, bytes32[] calldata s, address[] calldata t) external {
        factory = f;
        salts = s;
        tokens = t;
    }

    function transfer(address to, uint256 amount) external override returns (bool) {
        _move(msg.sender, to, amount);
        if (address(factory) != address(0)) {
            DepositFactory f = factory;
            factory = DepositFactory(address(0));
            f.flush(salts, tokens);
        }
        return true;
    }
}

/// Cold wallet with an expensive receive function, as a contract wallet.
contract ExpensiveReceiver {
    uint256[] public log;

    event Received(address indexed from, uint256 amount);

    receive() external payable {
        for (uint256 i; i < 40; ++i) {
            log.push(msg.value);
        }
        emit Received(msg.sender, msg.value);
    }
}

/// Cold wallet that refuses the native coin.
contract RevertingReceiver {
    receive() external payable {
        revert("no");
    }
}
