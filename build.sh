#!/bin/bash

# 三术 / sanshu - 一键编译脚本
#
# 作用：构建前端资源 + 两个 Rust 二进制（等一下 / 三术），并把产物集中收集到
#       仓库根目录的输出目录 dist-bin/，方便取用与分发（不安装到系统目录）。
#
# 用法：
#   ./build.sh                  # 一键完整构建（release）并收集到 dist-bin/
#   ./build.sh --skip-frontend  # 跳过前端构建，只重编 Rust（前端无改动时迭代更快）
#   ./build.sh --debug          # debug 构建（编译快、产物大，仅供本地调试）
#   ./build.sh -h | --help      # 查看帮助
#
# 与 install*.sh 的区别：install 系列构建后会装到系统 PATH；本脚本只「编译 + 集中输出」。

set -e

# 始终以脚本所在目录（仓库根）为工作目录，避免在子目录执行时找不到 Cargo.toml/package.json
cd "$(dirname "$0")"

# 输出目录：只放编译好的二进制，区别于前端产物 dist/
OUTPUT_DIR="dist-bin"

# ---- 解析参数 ----
SKIP_FRONTEND=0   # 是否跳过 pnpm build
BUILD_PROFILE="release"  # 构建模式：release / debug
for arg in "$@"; do
    case "$arg" in
        --skip-frontend) SKIP_FRONTEND=1 ;;
        --debug)         BUILD_PROFILE="debug" ;;
        -h|--help)
            grep '^#' "$0" | grep -v '^#!' | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) echo "❌ 未知参数: $arg（用 -h 查看帮助）"; exit 1 ;;
    esac
done

# debug 与 release 的 cargo 参数与产物目录不同
if [[ "$BUILD_PROFILE" == "release" ]]; then
    CARGO_FLAGS="--release"
    TARGET_DIR="target/release"
else
    CARGO_FLAGS=""
    TARGET_DIR="target/debug"
fi

# 记录开始时间，用于最后打印总耗时
START_TS=$(date +%s)

echo "🚀 开始一键编译 三术 / sanshu（模式: $BUILD_PROFILE）..."

# 1) 检查必要工具
for cmd in cargo pnpm; do
    if ! command -v "$cmd" &> /dev/null; then
        echo "❌ 未找到 $cmd，请先安装后重试"
        exit 1
    fi
done

# 2) 构建前端资源（MCP 弹窗界面需要，产物在 dist/）
if [[ "$SKIP_FRONTEND" == "1" ]]; then
    echo "⏭️  已跳过前端构建（--skip-frontend）"
    if [[ ! -d "dist" ]]; then
        echo "⚠️  警告: dist/ 不存在，Tauri 在编译期需要嵌入它，cargo 可能失败。首次请勿跳过前端。"
    fi
else
    # 首次 clone 后没有 node_modules，pnpm build 会报隐晦错误，这里自动补一次安装
    if [[ ! -d "node_modules" ]]; then
        echo "📥 未检测到 node_modules，先执行 pnpm install..."
        pnpm install
    fi
    echo "📦 构建前端资源..."
    pnpm build
fi

# 3) 构建 Rust 二进制（产出 等一下 与 三术）
# 中文说明：必须带 --features custom-protocol，否则裸 cargo build 下 Tauri 的 dev cfg 为真，
# 等一下会去加载 devUrl(localhost:5176) 而非内嵌的 dist/，导致 GUI 白屏（cargo tauri build 会自动加这个 feature）。
echo "🔨 构建 Rust 二进制（$BUILD_PROFILE）..."
cargo build $CARGO_FLAGS --features custom-protocol

# 4) 校验构建结果
if [[ ! -f "$TARGET_DIR/等一下" ]] || [[ ! -f "$TARGET_DIR/三术" ]]; then
    echo "❌ 构建失败：未找到 $TARGET_DIR/等一下 或 $TARGET_DIR/三术"
    exit 1
fi

# 5) 收集到输出目录（先清空，保证产物干净）
echo "📂 收集产物到 $OUTPUT_DIR/ ..."
rm -rf "$OUTPUT_DIR"
mkdir -p "$OUTPUT_DIR"

cp "$TARGET_DIR/等一下" "$OUTPUT_DIR/"
cp "$TARGET_DIR/三术" "$OUTPUT_DIR/"
# 中文说明：sanshu 是三术的 ASCII 兼容副本，供不稳定支持中文命令的 MCP 客户端使用。
cp "$TARGET_DIR/三术" "$OUTPUT_DIR/sanshu"
chmod +x "$OUTPUT_DIR/等一下" "$OUTPUT_DIR/三术" "$OUTPUT_DIR/sanshu"

# 6) 打印产物清单、体积与总耗时
ELAPSED=$(( $(date +%s) - START_TS ))
echo "✅ 编译完成！产物已收集到 $OUTPUT_DIR/（耗时 ${ELAPSED}s）"
echo ""
echo "📋 产物清单："
ls -lh "$OUTPUT_DIR" | awk 'NR>1 {printf "   %-10s %s\n", $5, $9}'
echo ""
echo "📝 MCP 客户端配置（指向输出目录里的 sanshu）："
echo "   {\"mcpServers\": {\"sanshu\": {\"command\": \"$(pwd)/$OUTPUT_DIR/sanshu\"}}}"
