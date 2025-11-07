#!/bin/bash

# 多链桥开发环境管理脚本

set -e

CONTAINER_NAME="multisig-bridge-dev"
PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# 颜色输出
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# 构建开发环境
build() {
    log_info "Building development environment..."
    cd "$PROJECT_DIR"
    docker compose build
    log_info "Build completed!"
}

# 启动开发环境
start() {
    log_info "Starting development environment..."
    cd "$PROJECT_DIR"
    
    # 检查容器是否已存在
    if docker ps -a --format '{{.Names}}' | grep -q "^${CONTAINER_NAME}$"; then
        log_warn "Container already exists. Starting..."
        docker start "$CONTAINER_NAME"
    else
        docker compose up -d
    fi
    
    log_info "Development environment started!"
    log_info "Use './scripts/dev.sh shell' to enter the container"
}

# 停止开发环境
stop() {
    log_info "Stopping development environment..."
    cd "$PROJECT_DIR"
    docker compose stop
    log_info "Development environment stopped!"
}

# 进入开发环境 Shell
shell() {
    log_info "Entering development environment shell..."
    docker exec -it "$CONTAINER_NAME" /bin/bash
}

# 查看日志
logs() {
    log_info "Showing logs..."
    docker logs -f "$CONTAINER_NAME"
}

# 清理环境
clean() {
    log_warn "This will remove the container and volumes. Continue? (y/N)"
    read -r response
    if [[ "$response" =~ ^[Yy]$ ]]; then
        log_info "Cleaning up..."
        cd "$PROJECT_DIR"
        docker compose down -v
        log_info "Cleanup completed!"
    else
        log_info "Cleanup cancelled."
    fi
}

# 重启环境
restart() {
    log_info "Restarting development environment..."
    stop
    sleep 2
    start
}

# 查看状态
status() {
    log_info "Container status:"
    docker ps -a --filter "name=$CONTAINER_NAME" --format "table {{.Names}}\t{{.Status}}\t{{.Ports}}"
}

# 帮助信息
help() {
    cat << EOF
多链桥开发环境管理脚本

使用方法:
  ./scripts/dev.sh <command>

命令:
  build      构建开发环境镜像
  start      启动开发环境 (后台运行)
  stop       停止开发环境
  restart    重启开发环境
  shell      进入开发环境 Shell
  logs       查看容器日志
  status     查看容器状态
  clean      清理环境 (删除容器和卷)
  help       显示此帮助信息

示例:
  ./scripts/dev.sh build     # 首次使用时构建环境
  ./scripts/dev.sh start     # 启动环境
  ./scripts/dev.sh shell     # 进入开发环境
EOF
}

# 主逻辑
case "${1:-}" in
    build)
        build
        ;;
    start)
        start
        ;;
    stop)
        stop
        ;;
    restart)
        restart
        ;;
    shell)
        shell
        ;;
    logs)
        logs
        ;;
    status)
        status
        ;;
    clean)
        clean
        ;;
    help|--help|-h)
        help
        ;;
    *)
        log_error "Unknown command: ${1:-}"
        echo ""
        help
        exit 1
        ;;
esac
