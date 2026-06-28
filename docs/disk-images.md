## Disk Images

> Disk images are created **automatically** by the builder. Just answer the prompts when running `cargo run`.

```
Disk Setup
  disk.img  ->  drive 1  (swap + ext4 root)
  data.img  ->  drive 3  (extra data storage, optional)

  disk.img size in MB (default 4096):
  Create data.img for extra storage? [y/N]:
  data.img size in MB (default 2048):
```

> If the images already exist, the builder reuses them without asking.
> To resize: delete the old `.img` files and re-run `cargo run`.

### QEMU Drive Layout

| Drive index | File | IDE bus | Purpose |
|---|---|---|---|
| **drive 1** | `disk.img` | `ide.0, unit=1` | swap + ext4 root |
| **drive 3** | `data.img` | `ide.1, unit=1` | extra data (optional) |

> Drive numbering: `ide.0 unit=0` = drive 0, `ide.0 unit=1` = drive 1, `ide.1 unit=0` = drive 2, `ide.1 unit=1` = drive 3.
> PCI block devices (AHCI / NVMe / virtio-blk) are probed at boot and get ids **4-7**.
> Every disk command (`gpt`, `mkfs.*`, `*mount`, `mkswap`, ...) accepts ids 0-7.

---