## Mounting Filesystems

MikuOS supports **2 simultaneous mount slots**. All ext commands operate on the **active slot**.

### Mount Commands

| Command | Syntax | Description |
|---|---|---|
| `ext2mount` | `ext2mount [drive] [partition]` | Mount ext2 filesystem |
| `ext3mount` | `ext3mount [drive] [partition]` | Mount ext3 filesystem |
| `ext4mount` | `ext4mount [drive] [partition]` | Mount ext4 filesystem |
| `fs.list` | `fs.list` | Show all mount slots and their status |
| `fs.select` | `fs.select <0\|1>` | Switch the active mount slot |
| `fs.umount` | `fs.umount [0\|1]` | Unmount a slot (default: active slot) |

### Examples

```bash
ext4mount 1 2           # mount partition 2 on drive 1
ext4mount 3             # mount entire drive 3
ext4mount               # auto-scan all drives
```

### Multi-Mount Workflow

```bash
ext4mount 1 2           # -> slot 0
ext4mount 3             # -> slot 1

fs.list
# [0] ext4 drive=1 lba=524322 free=773997/786432 (3022 MB)
# [1] ext4 drive=3 lba=0      free=515791/524287 (2014 MB) <- active

fs.select 0
extls /

fs.select 1
extls /
```

### Complete First-Boot Setup

```bash
gpt.init 1
gpt.add 1 swap 256
gpt.add 1 fs 3072

mkswap 1 1
mkfs.ext4 1 2
swapon 1 1
ext4mount 1 2

mkfs.ext4 3
ext4mount 3

fs.list
```

### After Reboot

```bash
swapon 1 1
ext4mount 1 2
ext4mount 3
```

---