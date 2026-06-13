## NVIDIA GPU Commands

> Native driver for GSP-era NVIDIA GPUs. The GTX 1650 family (TU116/TU117)
> gets the full pipeline (embedded FWSEC/SEC2 firmware, Falcon engines,
> DMA loopback, GSP-RM staging); every other Turing/Ampere/Ada card is
> recognized and probed host-side. See README for driver internals.

| Command | Description |
|---|---|
| `nvidia` / `nvidia info` | Show detected GPU: chip, VRAM, BARs, state |
| `nvidia list` | List all detected NVIDIA GPUs |
| `nvidia debug` | Dump key MMIO registers |
| `nvidia firmware` | Firmware bundle status (`/lib/firmware` via fwload) |
| `nvidia falcon` | Falcon engine liveness check |
| `nvidia ungate` | Ungate engine clocks |
| `nvidia temp` | GPU temperature |
| `nvidia dma-test` | SEC2 DMEM DMA loopback self-test |
| `nvidia acr` | ACR / WPR carveout info |
| `nvidia gsp` | GSP processor state |
| `nvidia gsp-rm` | GSP-RM offload pipeline status |
| `nvidia gsp-rm-dryrun` | Stage GSP-RM boot without executing |
| `nvidia gsp-rm-load` / `gsp-rm-boot` | Load / boot GSP-RM firmware |
| `nvidia scan` | PMC device scan |

---