#!/bin/bash

# 注册 Solana 中继器
timeout 10s npx ts-node svm-admin.ts add_relayer ZUPTJgxipfzYX6bRTSQa3wFE1h4zz9DHs7ivviJy2nm
timeout 10s npx ts-node svm-admin.ts add_relayer 2ZyzfnfXFW1gN4rp4mGgXrYSBSKL8KeeaTAhWmzzkvjM
timeout 10s npx ts-node svm-admin.ts add_relayer EgajGAu5uJebnpTu5eaAjiNkqv9MUwExhqRc97E6pxeW

# 注册 EVM 中继器
npx ts-node evm-admin.ts add_relayer 0xce0F6bE5d09FeECf37C673D954666250E0373772
npx ts-node evm-admin.ts add_relayer 0x0D5574C4A07eBb54D5501F85ED6464726022C9C0
npx ts-node evm-admin.ts add_relayer 0xFD8da8d7EFd1a83e9Fb8DED6Ec6921d1207C06CF
