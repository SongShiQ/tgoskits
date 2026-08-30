#!/bin/sh
# 板级测试入口：StarryOS shell 下由 init.sh 调用。
# 逐级验证并在全部通过时打印 SENSEVOICE_RKNN_TEST_PASSED。

PY=/opt/sensevoice/bin/python3
MODEL_DIR=/opt/sensevoice/model
WAV_DIR=/opt/sensevoice/testwavs
export LD_LIBRARY_PATH=/opt/sensevoice/lib:/usr/lib:/lib
export PYTHONPATH=/opt/sensevoice/python

run="$PY /opt/sensevoice/python/sensevoice_rknn_npu.py --model-dir $MODEL_DIR --lib-dir /opt/sensevoice/lib"

# L0: interpreter + imports
# NOTE: capture rc before any other command runs. Writing `if ! cmd; then echo $?`
# reports the status of the negation (always 0), hiding the real code (e.g. 134 on SIGABRT).
$PY /opt/sensevoice/python/sensevoice_rknn_npu.py --help >/tmp/sv0.log 2>&1
rc=$?
if [ $rc -ne 0 ]; then
    echo "SENSEVOICE_RKNN_TEST_FAILED: L0 rc=$rc"
    cat /tmp/sv0.log
    exit 1
fi

# L1: missing model -> nonzero exit + diagnostic
if $run --selftest --wav /nonexistent.wav >/tmp/sv1.log 2>&1; then
    echo "SENSEVOICE_RKNN_TEST_FAILED: L1 missing model returned 0"
    exit 1
fi
grep -qiE "not found|failed|error" /tmp/sv1.log || {
    echo "SENSEVOICE_RKNN_TEST_FAILED: L1 no diagnostic"
    exit 1
}

# L2: zh reference clip (skipped if test.wav is not staged)
if [ ! -f "$WAV_DIR/test.wav" ]; then
    echo "SENSEVOICE_RKNN_TEST_SKIP: L2 (test.wav not staged)"
else
    $run --wav $WAV_DIR/test.wav >/tmp/sv2.out 2>/tmp/sv2.log
    rc=$?
    if [ $rc -ne 0 ]; then
        echo "SENSEVOICE_RKNN_TEST_FAILED: L2 rc=$rc"
        tail -5 /tmp/sv2.log
        exit 1
    fi
    echo "L2 transcript: $(cat /tmp/sv2.out)"
    grep -q "开饭时间早上9点至下午5点" /tmp/sv2.out || {
        echo "SENSEVOICE_RKNN_TEST_FAILED: L2 transcript mismatch"
        cat /tmp/sv2.out
        exit 1
    }
fi

# L3: en reference clip (skipped if en.wav is not staged)
if [ ! -f "$WAV_DIR/en.wav" ]; then
    echo "SENSEVOICE_RKNN_TEST_SKIP: L3 (en.wav not staged)"
else
    $run --wav $WAV_DIR/en.wav >/tmp/sv3.out 2>/tmp/sv3.log
    rc=$?
    if [ $rc -ne 0 ]; then
        echo "SENSEVOICE_RKNN_TEST_FAILED: L3 rc=$rc"
        tail -5 /tmp/sv3.log
        exit 1
    fi
    echo "L3 transcript: $(cat /tmp/sv3.out)"
    grep -qi "the tribal chieftain" /tmp/sv3.out || {
        echo "SENSEVOICE_RKNN_TEST_FAILED: L3 transcript mismatch"
        cat /tmp/sv3.out
        exit 1
    }
fi

# Report perf only if L2 actually ran (sv2.log exists).
if [ -f /tmp/sv2.log ] && grep -q "\[perf\] model load" /tmp/sv2.log; then
    grep "\[perf\]" /tmp/sv2.log
fi
echo "SENSEVOICE_RKNN_TEST_PASSED"
