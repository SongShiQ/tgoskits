# SenseVoice RKNN on StarryOS (Orange Pi 5 Plus)

中文语音识别 + 控制：StarryOS `/dev/audio0` 采集 → SenseVoice ASR（RK3588 NPU）→ 指令解析 → RT 控车。

## 现状：以 StarryOS 为基础，补全了什么

StarryOS 工作分支 `audio/rk3588-capture` 已有三层基础设施：

1. **NPU 内核驱动**（`drivers/npu/rockchip-npu/`）+ DRM 用户态接口（`/dev/dri/card1`，
   `os/StarryOS/kernel/src/pseudofs/dev/card1.rs`，实现 RknpuSubmit/MemCreate/MemMap 等 ioctl）。
2. **SenseVoice 推理脚本**（`sensevoice_rknn_npu.py`，627 行）：librknnrt C API via ctypes，
   CPU 侧 fbank80 + LFR + CMVN + CTC 解码，只需 numpy。
3. **音频采集**（`/dev/audio0`）：麦克风 → ES8388 → I2S RX → PCM ring → 用户态，已验证。

本 app 补全的是"把上述积木组装成可上板运行的 rootfs overlay"：

- `prebuild.sh`：用 qemu-user 从 Alpine base rootfs 装 python3 + numpy，复制 .so 闭包；
  复用 uvc-rknn app 已验证的 `librknnrt.so`（aarch64）；从 HuggingFace 下载 RK3588
  fp16-scaled 模型 + 解码资源到 `/opt/sensevoice/`。
- `init.sh` / `board-orangepi-5-plus.toml`：板端启动时跑 `sensevoice-rknn-test.sh` 的
  L0-L3 自测，期望打印 `SENSEVOICE_RKNN_TEST_PASSED`。

## rootfs overlay 布局

```
/opt/sensevoice/
├── bin/python3                    -> /usr/bin/python3
├── lib/librknnrt.so                aarch64 RKNN runtime (复用 uvc-rknn 3rdparty)
├── model/
│   ├── sense-voice-encoder.rk3588.fp16-scaled.rknn   (harvestsu/sensevoice-rknn)
│   ├── embedding.npy
│   ├── am.mvn
│   ├── chn_jpn_yue_eng_ko_spectok.bpe.model
│   └── tokens.txt                  (从 sherpa-onnx tarball 提取)
├── python/sensevoice_rknn_npu.py   (仓库根目录的 ASR 脚本)
├── sensevoice-rknn-test.sh          (L0-L3 自测入口)
└── testwavs/test.wav, en.wav        (参考音频)
/usr/bin/python3 + numpy 闭包        (Alpine apk 预装)
```

## 模型来源

| 资源 | 仓库 | 说明 |
|------|------|------|
| `sense-voice-encoder.rk3588.fp16-scaled.rknn` | `harvestsu/sensevoice-rknn` | RK3588 专用 fp16-scaled 模型 |
| `embedding.npy` / `am.mvn` / `*.bpe.model` | `harvestsu/sensevoice-rknn` | 解码资源 |
| `tokens.txt` | `happyme531/SenseVoiceSmall-RKNN2` 内 sherpa tarball | id→token 表 |
| `librknnrt.so` | 仓库内 `apps/starry/orangepi-5-plus-uvc-rknn/.../rknpu2/Linux/aarch64/` | 已验证可用 |

## 测试层级（sensevoice-rknn-test.sh）

- **L0**：python3 + numpy + ctypes 导入成功（`--help` 退出码 0）
- **L1**：缺模型文件时非零退出且有诊断信息
- **L2**：中文参考 wav 识别出 `开饭时间早上九点至下午五点`
- **L3**：英文参考 wav 识别出 `the tribal chieftain`
- 全部通过打印 `SENSEVOICE_RKNN_TEST_PASSED`

## 构建

```bash
# 在 WSL /root/work/tgoskits-audio 下
# 构建会自动跑 prebuild.sh 下载模型 + 装 python/numpy + 注入 overlay
make build APP=sensevoice-rknn BOARD=orangepi-5-plus   # 或对应的 axbuild 命令
```

## 待办：实时采集接入

当前 L2/L3 用离线 wav 验证识别链路。实时语音控制还需：
1. `/dev/audio0` 采 48k/2ch/S16 → 重采样 16k/单声道
2. VAD 断句 → 喂 `sensevoice_rknn_npu.py` 的 `transcribe()`（目前只吃 wav）
3. 识别文本 → `voice_commands_from_text()` → `send_voice_command()` 经 `/dev/console` 发 RT 指令

`sensevoice_rknn_npu.py` 已内置 `voice_commands_from_text` / `send_voice_command` 逻辑，
离线链路跑通后只需补录音→特征管线。
## 实时模式（--live）

`sensevoice_rknn_npu.py` 增加了 `--live` 参数，打通实时链路：

```
/dev/audio0 (48k/mono/S16) → 重采样 16k → VAD 断句 → NPU ASR → 指令解析 → /dev/console 控车
```

板端运行：
```bash
export LD_LIBRARY_PATH=/opt/sensevoice/lib
/opt/sensevoice/bin/python3 /opt/sensevoice/python/sensevoice_rknn_npu.py --live --language auto
```

VAD 可调（应对板端话筒灵敏度低）：
- `SENSEVOICE_VAD_THRESHOLD=0.006`（默认 0.012，调低更灵敏）
- `SENSEVOICE_VAD_SILENCE=1.0`（静音超时秒数）
- `SENSEVOICE_VAD_MIN_SPEECH=0.3`（最短语音秒数）

qemu-user 已验证：重采样 480→160 样本正确、VAD 检测出 1 个 utterance、`transcribe_samples`/`run_live` 可调用。真机 NPU 推理待上板验证。