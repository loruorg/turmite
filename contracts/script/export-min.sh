#!/bin/sh
# Writes out-min/DepositFactory.json from the forge build output. Run `forge build` first. Needs jq.
set -eu
cd "$(dirname "$0")/.."
F=out/DepositFactory.sol/DepositFactory.json
W=out/DepositWallet.sol/DepositWallet.json
mkdir -p out-min
jq -n --slurpfile f "$F" --slurpfile w "$W" '
{
  contractName: "DepositFactory",
  compiler: {
    solc: $f[0].metadata.compiler.version,
    evmVersion: $f[0].metadata.settings.evmVersion,
    optimizerRuns: $f[0].metadata.settings.optimizer.runs,
    bytecodeHash: $f[0].metadata.settings.metadata.bytecodeHash
  },
  abi: $f[0].abi,
  bytecode: $f[0].bytecode.object,
  deployedBytecode: $f[0].deployedBytecode.object,
  implementation: {
    contractName: "DepositWallet",
    abi: $w[0].abi,
    bytecode: $w[0].bytecode.object
  },
  create2Deployer: "0x4e59b44847b379578588920ca78fbf26c0b4956c",
  factorySalt: "0x0000000000000000000000000000000000000000000000000000000000000000",
  cloneInitCodePrefix: "0x3d602d80600a3d3981f3363d3d373d3d3d363d73",
  cloneInitCodeSuffix: "0x5af43d82803e903d91602b57fd5bf3",
  formulas: {
    factory: "keccak256(0xff ++ create2Deployer ++ factorySalt ++ keccak256(bytecode ++ abi.encode(coldWallet)))[12:]",
    implementation: "keccak256(rlp([factory, 1]))[12:] (CREATE by the factory at nonce 1)",
    cloneInitCode: "cloneInitCodePrefix ++ implementation ++ cloneInitCodeSuffix (55 bytes, EIP-1167)",
    depositAddress: "keccak256(0xff ++ factory ++ salt ++ keccak256(cloneInitCode))[12:]",
    salt: "bytes32(uint256(payment_id)), big-endian"
  }
}' > out-min/DepositFactory.json
echo "wrote out-min/DepositFactory.json"
