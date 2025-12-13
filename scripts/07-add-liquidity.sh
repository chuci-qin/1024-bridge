#!/bin/bash

npx ts-node evm-admin.ts add_liquidity 100
timeout 10s npx ts-node svm-admin.ts add_liquidity 100000000
