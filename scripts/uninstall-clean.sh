#!/bin/sh
# everytxt 卸载清理脚本（批次 C / M4：手册 7.12 卸载删除所有数据）
#
# 背景：Tauri v2 的 deb 打包不支持自定义 postrm maintainer 脚本，
# 且 Debian 惯例是 `apt remove` 保留 $HOME 用户数据（与手册 7.12 的
# 「卸载删除所有数据」语义冲突）。本脚本随安装包分发到
# /usr/bin/uninstall-clean.sh，供卸载后手动执行一次完成彻底清理。
#
# 用法：
#   sudo apt remove search-tool   # 先卸载程序本体
#   uninstall-clean.sh            # 再清数据（或删除本文件前运行）
#
# 清理范围（与 state.rs default_index_dir 一致）：
#   ~/.local/share/search-tool/   索引库 + app.json（主题/语言/热键/缩放）

set -e

DATA_DIR="${HOME}/.local/share/search-tool"

if [ -d "$DATA_DIR" ]; then
    rm -rf "$DATA_DIR"
    echo "已删除用户数据：$DATA_DIR"
else
    echo "未发现用户数据目录（$DATA_DIR 不存在），无需清理。"
fi
