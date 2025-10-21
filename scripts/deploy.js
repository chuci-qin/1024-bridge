const hre = require("hardhat");

async function main() {
  console.log("🚀 部署1024Bridge合约...");
  
  // USDC地址（根据网络选择）
  const network = hre.network.name;
  let usdcAddress;
  
  if (network === "arbitrumSepolia") {
    // Arbitrum Sepolia测试网USDC（需要确认实际地址）
    usdcAddress = process.env.USDC_ADDRESS_SEPOLIA || "0x...";
    console.log("📍 网络: Arbitrum Sepolia");
  } else if (network === "arbitrumOne") {
    // Arbitrum One主网USDC
    usdcAddress = "0xaf88d065e77c8cC2239327C5EDb3A432268e5831";
    console.log("📍 网络: Arbitrum One");
  } else {
    console.log("📍 网络: Hardhat本地");
    // 本地测试：部署Mock USDC
    const MockUSDC = await hre.ethers.getContractFactory("MockUSDC");
    const mockUSDC = await MockUSDC.deploy();
    await mockUSDC.waitForDeployment();
    usdcAddress = await mockUSDC.getAddress();
    console.log("✅ Mock USDC deployed:", usdcAddress);
  }
  
  console.log("💰 USDC地址:", usdcAddress);
  
  // 部署Bridge1024
  const Bridge = await hre.ethers.getContractFactory("Bridge1024");
  const bridge = await Bridge.deploy(usdcAddress);
  
  await bridge.waitForDeployment();
  
  const bridgeAddress = await bridge.getAddress();
  
  console.log("✅ 1024Bridge deployed to:", bridgeAddress);
  console.log("");
  console.log("📋 部署信息:");
  console.log("   网络:", network);
  console.log("   USDC:", usdcAddress);
  console.log("   Bridge:", bridgeAddress);
  console.log("");
  console.log("🔗 Arbiscan:", `https://${network === 'arbitrumOne' ? '' : 'sepolia.'}arbiscan.io/address/${bridgeAddress}`);
  console.log("");
  console.log("🔑 下一步:");
  console.log("   1. 验证合约: npx hardhat verify --network", network, bridgeAddress, usdcAddress);
  console.log("   2. 添加Relayer: bridge.addRelayer(relayerAddress)");
  console.log("   3. 添加Guardian: bridge.grantRole(GUARDIAN_ROLE, guardianAddress)");
}

main()
  .then(() => process.exit(0))
  .catch((error) => {
    console.error(error);
    process.exit(1);
  });

