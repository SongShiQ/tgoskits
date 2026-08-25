# RK3588 Orange Pi 5 Plus Capture Design

Status: design and host-side core only. The register values below are not a claim that the board has been exercised.

## Hardware evidence

The Orange Pi 5 Plus Linux device tree in `drivers/npu/rockchip-npu/fireware/orangepi5plus.dts` identifies the active board codec path as:

| Function | Evidence |
| --- | --- |
| CPU DAI | `i2s@fe470000`, compatible `rockchip,rk3588-i2s-tdm` |
| MMIO | `0xfe470000`, size `0x1000` |
| IRQ | GIC SPI `0xb4` |
| DMA | controller/channel pairs `0x7a/0x00` and `0x7a/0x01` |
| Codec | `es8388@11`, I2C address `0x11` |
| Link | `rockchip,multicodecs-card`, I2S format, MCLK ratio `256` |
| Capture routes | `LINPUT1/LINPUT2 -> Main Mic`, `RINPUT1/RINPUT2 -> Headset Mic` |

The separate `i2s@fddfc000` capture-only node is not used as the first target because the board sound card points at `fe470000`.

## Linux register evidence

The register offsets and bit fields are taken from Linux v6.1 `sound/soc/rockchip/rockchip_i2s_tdm.h` and the setup order from `rockchip_i2s_tdm.c`:

- `TXCR 0x000`, `RXCR 0x004`, `CKR 0x008`, `DMACR 0x010`, `INTCR 0x014`, `INTSR 0x018`, `XFER 0x01c`, `CLR 0x020`, `RXFIFOLR 0x02c`.
- RX DMA enable is `DMACR.RDE` bit 24; RX DMA level is bits 16-20.
- RX FIFO threshold enable is `INTCR.RXFIE` bit 16; RX overflow interrupt enable is bit 17; RX overflow clear is bit 18.
- RX FIFO and overflow status are reported at `INTSR` bits 16 and 17.
- RX transfer starts with `XFER.RXS_START` bit 1 and is cleared with `CLR_RXC` bit 1.

The driver core now uses these offsets and fields. Clock parent selection, exact divider values, I2S master/slave role, slot count, and the ES8388 register sequence remain board-glue responsibilities until the Linux boot logs and schematic are checked on the actual board.

## Layer boundary

```text
StarryOS /dev/audio0
    -> audio runtime: blocking read, poll, ioctl, user copy
    -> RK3588 board glue: FDT, clocks, reset, pinctrl, I2C, IRQ, DMA API
    -> rockchip-i2s-tdm core: register sequence, IRQ event, PCM ring
    -> RK3588 I2S/TDM + ES8388
```

The first public format is logical mono, signed 16-bit PCM at 16 kHz or 48 kHz. If the codec/DAI delivers two slots, the runtime will select or downmix one slot before publishing mono samples; this is intentionally not hidden in the register core.

## Validation gates

1. Host unit tests for format validation and ring wrap/overrun.
2. Mock-MMIO tests for write order and interrupt acknowledgement.
3. Linux board probe to confirm clocks, codec and actual `dmesg`/`/proc/asound` topology.
4. StarryOS DMA and IRQ integration.
5. `/dev/audio0` WAV capture and 10-minute stability run.
