#!/bin/bash
# cgroup v2 基础改进测试用例

set -e

echo "=== cgroup v2 基础改进测试 ==="

# 测试目录
TEST_CGROUP="/cgroup/test_$$"
mkdir -p "$TEST_CGROUP"

# 测试 1: cgroup.type
echo "测试 1: cgroup.type"
echo "---"
# 读取默认类型
DEFAULT_TYPE=$(cat "$TEST_CGROUP/cgroup.type")
echo "默认类型: $DEFAULT_TYPE"
if [ "$DEFAULT_TYPE" = "domain" ]; then
    echo "✓ 默认类型正确"
else
    echo "✗ 默认类型错误: 期望 'domain'，实际 '$DEFAULT_TYPE'"
    exit 1
fi

# 写入有效类型
echo "domain" > "$TEST_CGROUP/cgroup.type"
echo "✓ 写入 'domain' 成功"

# 写入无效类型（应该失败）
if echo "invalid" > "$TEST_CGROUP/cgroup.type" 2>/dev/null; then
    echo "✗ 写入无效类型应该失败"
    exit 1
else
    echo "✓ 写入无效类型正确失败"
fi

# 测试 2: cgroup.events
echo ""
echo "测试 2: cgroup.events"
echo "---"
# 空 cgroup 的 events
EVENTS=$(cat "$TEST_CGROUP/cgroup.events")
echo "空 cgroup events:"
echo "$EVENTS"
if echo "$EVENTS" | grep -q "populated 0"; then
    echo "✓ 空 cgroup populated 正确"
else
    echo "✗ 空 cgroup populated 错误"
    exit 1
fi

if echo "$EVENTS" | grep -q "frozen 0"; then
    echo "✓ 空 cgroup frozen 正确"
else
    echo "✗ 空 cgroup frozen 错误"
    exit 1
fi

# 测试 3: cgroup.freeze
echo ""
echo "测试 3: cgroup.freeze"
echo "---"
# 读取默认冻结状态
FREEZE_STATE=$(cat "$TEST_CGROUP/cgroup.freeze")
echo "默认冻结状态: $FREEZE_STATE"
if [ "$FREEZE_STATE" = "0" ]; then
    echo "✓ 默认冻结状态正确"
else
    echo "✗ 默认冻结状态错误"
    exit 1
fi

# 冻结
echo "1" > "$TEST_CGROUP/cgroup.freeze"
FREEZE_STATE=$(cat "$TEST_CGROUP/cgroup.freeze")
if [ "$FREEZE_STATE" = "1" ]; then
    echo "✓ 冻结成功"
else
    echo "✗ 冻结失败"
    exit 1
fi

# 检查 events 中的 frozen
EVENTS=$(cat "$TEST_CGROUP/cgroup.events")
if echo "$EVENTS" | grep -q "frozen 1"; then
    echo "✓ events.frozen 正确更新"
else
    echo "✗ events.frozen 未更新"
    exit 1
fi

# 解冻
echo "0" > "$TEST_CGROUP/cgroup.freeze"
FREEZE_STATE=$(cat "$TEST_CGROUP/cgroup.freeze")
if [ "$FREEZE_STATE" = "0" ]; then
    echo "✓ 解冻成功"
else
    echo "✗ 解冻失败"
    exit 1
fi

# 测试 4: cgroup.kill
echo ""
echo "测试 4: cgroup.kill"
echo "---"
# 杀死空 cgroup（应该成功）
if echo "1" > "$TEST_CGROUP/cgroup.kill" 2>/dev/null; then
    echo "✓ 杀死空 cgroup 成功"
else
    echo "✗ 杀死空 cgroup 失败"
    exit 1
fi

# 测试 5: subtree_control
echo ""
echo "测试 5: subtree_control"
echo "---"
# 空 cgroup 启用控制器
echo "+pids" > "$TEST_CGROUP/cgroup.subtree_control"
CONTROL=$(cat "$TEST_CGROUP/cgroup.subtree_control")
if echo "$CONTROL" | grep -q "pids"; then
    echo "✓ 启用 pids 控制器成功"
else
    echo "✗ 启用 pids 控制器失败"
    exit 1
fi

# 禁用控制器
echo "-pids" > "$TEST_CGROUP/cgroup.subtree_control"
CONTROL=$(cat "$TEST_CGROUP/cgroup.subtree_control")
if echo "$CONTROL" | grep -q "pids"; then
    echo "✗ 禁用 pids 控制器失败"
    exit 1
else
    echo "✓ 禁用 pids 控制器成功"
fi

# 测试 6: procfs cgroup
echo ""
echo "测试 6: procfs cgroup"
echo "---"
# 读取当前进程的 cgroup
PROC_CGROUP=$(cat /proc/self/cgroup)
echo "当前进程 cgroup: $PROC_CGROUP"
if echo "$PROC_CGROUP" | grep -q "0::"; then
    echo "✓ procfs cgroup 格式正确"
else
    echo "✗ procfs cgroup 格式错误"
    exit 1
fi

# 测试 7: 边界条件
echo ""
echo "测试 7: 边界条件"
echo "---"
# 有进程时启用控制器（应该失败）
echo "当前 shell 加入测试 cgroup"
echo $$ > "$TEST_CGROUP/cgroup.procs"

if echo "+pids" > "$TEST_CGROUP/cgroup.subtree_control" 2>/dev/null; then
    echo "✗ 有进程时启用控制器应该失败"
    exit 1
else
    echo "✓ 有进程时启用控制器正确失败"
fi

# 验证 populated 事件
EVENTS=$(cat "$TEST_CGROUP/cgroup.events")
if echo "$EVENTS" | grep -q "populated 1"; then
    echo "✓ populated 事件正确更新"
else
    echo "✗ populated 事件未更新"
    exit 1
fi

# 清理
echo ""
echo "清理测试环境"
echo $$ > /cgroup/cgroup.procs
rmdir "$TEST_CGROUP" 2>/dev/null || true

echo ""
echo "=== 所有测试通过 ==="
