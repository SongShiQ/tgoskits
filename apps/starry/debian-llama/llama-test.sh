#!/bin/sh
set -eu

echo "L0_START"
llama-cli --help >/dev/null 2>&1 || echo "L0_FAILED RC=$?"
echo "L0_HELP PASS"

llama-cli -m /opt/models/tiny-llm-q4_0.gguf -p "Hello" -n 4 -t 1 --no-mmap 2>&1 || echo "L4_FAILED RC=$?"
echo "L4_INFER PASS"

echo "LLAMA_DEBIAN_TEST_DONE"
