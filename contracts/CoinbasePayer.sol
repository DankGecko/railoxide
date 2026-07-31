// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

contract CoinbasePayer {
    error CoinbasePaymentFailed();

    receive() external payable {
        (bool success,) = payable(block.coinbase).call{value: msg.value}("");
        if (!success) revert CoinbasePaymentFailed();
    }
}
