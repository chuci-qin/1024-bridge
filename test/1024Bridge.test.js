const { expect } = require("chai");
const { ethers } = require("hardhat");

describe("1024Bridge", function () {
  let bridge;
  let usdc;
  let owner;
  let relayer;
  let user;
  
  beforeEach(async function () {
    [owner, relayer, user] = await ethers.getSigners();
    
    // 部署Mock USDC
    const MockUSDC = await ethers.getContractFactory("MockUSDC");
    usdc = await MockUSDC.deploy();
    await usdc.waitForDeployment();
    
    // 部署Bridge
    const Bridge = await ethers.getContractFactory("1024Bridge");
    bridge = await Bridge.deploy(await usdc.getAddress());
    await bridge.waitForDeployment();
    
    // 设置Relayer角色
    const RELAYER_ROLE = await bridge.RELAYER_ROLE();
    await bridge.grantRole(RELAYER_ROLE, relayer.address);
    
    // 给用户一些USDC
    await usdc.mint(user.address, ethers.parseUnits("10000", 6));
  });
  
  describe("充值功能", function () {
    it("应该成功锁定USDC", async function () {
      const amount = ethers.parseUnits("100", 6);  // 100 USDC
      const solanaAddress = "DjUKDJ5XMrdTyjjFikmWamKwsAQtNF1mTYkvDEJmYpud";
      
      // 授权
      await usdc.connect(user).approve(await bridge.getAddress(), amount);
      
      // 锁定USDC
      const tx = await bridge.connect(user).lockUSDC(amount, solanaAddress);
      const receipt = await tx.wait();
      
      // 验证事件
      const event = receipt.logs.find(log => log.eventName === "USDCLocked");
      expect(event).to.not.be.undefined;
      expect(event.args.user).to.equal(user.address);
      expect(event.args.solanaAddress).to.equal(solanaAddress);
      
      // 验证USDC已转入合约
      const bridgeBalance = await usdc.balanceOf(await bridge.getAddress());
      expect(bridgeBalance).to.be.gt(0);
    });
    
    it("应该拒绝超过限额的充值", async function () {
      const amount = ethers.parseUnits("200000", 6);  // 200K USDC（超过限额）
      const solanaAddress = "DjUKDJ5XMrdTyjjFikmWamKwsAQtNF1mTYkvDEJmYpud";
      
      await usdc.connect(user).approve(await bridge.getAddress(), amount);
      
      await expect(
        bridge.connect(user).lockUSDC(amount, solanaAddress)
      ).to.be.revertedWith("Exceeds single tx limit");
    });
  });
  
  describe("提现功能", function () {
    it("Relayer应该可以解锁USDC", async function () {
      // 先充值一些USDC到合约
      const depositAmount = ethers.parseUnits("1000", 6);
      await usdc.mint(await bridge.getAddress(), depositAmount);
      
      const withdrawAmount = ethers.parseUnits("100", 6);
      const burnTxHash = ethers.keccak256(ethers.toUtf8Bytes("test_burn_tx_123"));
      
      // Relayer解锁
      await bridge.connect(relayer).unlockUSDC(
        user.address,
        withdrawAmount,
        burnTxHash
      );
      
      // 验证用户收到USDC
      const userBalance = await usdc.balanceOf(user.address);
      expect(userBalance).to.be.gte(withdrawAmount);
    });
    
    it("应该防止重放攻击", async function () {
      await usdc.mint(await bridge.getAddress(), ethers.parseUnits("1000", 6));
      
      const amount = ethers.parseUnits("100", 6);
      const burnTxHash = ethers.keccak256(ethers.toUtf8Bytes("test_burn_456"));
      
      // 第一次解锁
      await bridge.connect(relayer).unlockUSDC(user.address, amount, burnTxHash);
      
      // 第二次应该失败（重放攻击）
      await expect(
        bridge.connect(relayer).unlockUSDC(user.address, amount, burnTxHash)
      ).to.be.revertedWith("Already processed");
    });
    
    it("非Relayer不能解锁", async function () {
      await usdc.mint(await bridge.getAddress(), ethers.parseUnits("1000", 6));
      
      const amount = ethers.parseUnits("100", 6);
      const burnTxHash = ethers.keccak256(ethers.toUtf8Bytes("test_burn_789"));
      
      // 普通用户尝试解锁应该失败
      await expect(
        bridge.connect(user).unlockUSDC(user.address, amount, burnTxHash)
      ).to.be.reverted;
    });
  });
  
  describe("管理功能", function () {
    it("Guardian应该可以暂停", async function () {
      await bridge.pause();
      
      const amount = ethers.parseUnits("100", 6);
      await usdc.connect(user).approve(await bridge.getAddress(), amount);
      
      await expect(
        bridge.connect(user).lockUSDC(amount, "TestAddress")
      ).to.be.revertedWith("Pausable: paused");
    });
  });
  
  describe("LP功能（Phase 3预留）", function () {
    it("Phase 2应该revert LP功能", async function () {
      const amount = ethers.parseUnits("1000", 6);
      
      await expect(
        bridge.connect(user).stakeLiquidity(amount)
      ).to.be.revertedWith("Phase 3 feature");
    });
  });
});

