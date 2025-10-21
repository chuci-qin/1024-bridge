# Contributing to 1024 Bridge

Thank you for your interest in contributing to 1024 Bridge! 🎉

---

## 🌟 Ways to Contribute

- 🐛 Report bugs
- 💡 Suggest features
- 📝 Improve documentation
- 🔧 Submit code improvements
- 🧪 Add tests

---

## 🚀 Getting Started

### 1. Fork and Clone

```bash
git clone https://github.com/YOUR-USERNAME/1024-bridge.git
cd 1024-bridge
```

### 2. Install Dependencies

```bash
npm install
```

### 3. Create a Branch

```bash
git checkout -b feature/your-feature-name
```

---

## 📋 Development Workflow

### 1. Make Changes

- Write clean, documented code
- Follow existing code style
- Add tests for new features

### 2. Test Your Changes

```bash
# Compile
npx hardhat compile

# Run tests
npx hardhat test

# Check coverage
npx hardhat coverage
```

### 3. Commit

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```bash
git commit -m "feat: Add liquidity mining support"
git commit -m "fix: Resolve reentrancy vulnerability"
git commit -m "docs: Update architecture diagram"
```

### 4. Push and PR

```bash
git push origin feature/your-feature-name
```

Then open a Pull Request on GitHub.

---

## ✅ Code Quality Standards

### Solidity

- **Version**: ^0.8.20
- **Style**: Follow [Solidity Style Guide](https://docs.soliditylang.org/en/latest/style-guide.html)
- **Security**: Use OpenZeppelin contracts
- **Comments**: NatSpec for all public functions

### Testing

- **Coverage**: Aim for >90%
- **Test Cases**: Include edge cases
- **Gas Optimization**: Check gas usage

---

## 🔍 Pull Request Process

1. **Update Documentation**: If you change functionality
2. **Add Tests**: For new features
3. **Pass CI**: All tests must pass
4. **Code Review**: At least 1 approval required
5. **Squash Commits**: Before merging

---

## 🐛 Reporting Bugs

### Security Vulnerabilities

**DO NOT** open public issues for security bugs.

Email: security@1024.exchange

### Non-Security Bugs

Open an issue with:
- **Title**: Clear, concise description
- **Description**: Steps to reproduce
- **Expected**: What should happen
- **Actual**: What actually happens
- **Environment**: Network, versions, etc.

---

## 💡 Feature Requests

Open an issue with:
- **Use Case**: Why is this needed?
- **Proposal**: How should it work?
- **Alternatives**: Other solutions considered?

---

## 📜 Code of Conduct

Be respectful, inclusive, and professional.

See [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) for details.

---

## 📚 Resources

- **Documentation**: https://docs.1024.exchange/bridge
- **Discord**: https://discord.gg/1024exchange
- **Architecture**: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)

---

## 🙏 Thank You!

Your contributions make 1024 Bridge better for everyone! 🚀

