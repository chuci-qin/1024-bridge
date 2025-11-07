#!/bin/bash

# 启动19个Guardian网络

set -e

GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_section() {
    echo -e "\n${BLUE}═══ $1 ═══${NC}\n"
}

cd /workspace

log_section "启动 Guardian 网络"

log_info "检查配置..."
if [ ! -f docker-compose.guardian.yml ]; then
    echo "错误: docker-compose.guardian.yml 不存在"
    exit 1
fi

log_info "启动19个Guardian节点..."
docker-compose -f docker-compose.guardian.yml up -d

log_info "等待节点启动..."
sleep 10

log_info "检查节点状态..."
RUNNING=$(docker-compose -f docker-compose.guardian.yml ps --services --filter "status=running" | wc -l)

log_section "Guardian 网络状态"

echo "运行中的节点: $RUNNING/19"
echo ""
echo "API 端点:"
echo "  Guardian 1:  http://localhost:7071"
echo "  Guardian 2:  http://localhost:7072"
echo "  Guardian 3:  http://localhost:7073"
echo "  ..."
echo "  Guardian 19: http://localhost:7089"
echo ""
echo "查看日志:"
echo "  docker-compose -f docker-compose.guardian.yml logs -f guardian-1"
echo ""
echo "停止网络:"
echo "  docker-compose -f docker-compose.guardian.yml down"

