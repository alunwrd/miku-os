## Formatting

After partitioning, format each partition before use.

### mkfs Commands

| Command | Syntax | Description |
|---|---|---|
| `mkfs.ext2` | `mkfs.ext2 <drive> [partition]` | Format as ext2 |
| `mkfs.ext3` | `mkfs.ext3 <drive> [partition]` | Format as ext3 (with journal) |
| `mkfs.ext4` | `mkfs.ext4 <drive> [partition]` | Format as ext4 (journal + extents) |
| `mkfs.dry` | `mkfs.dry <drive> <ext2\|ext3\|ext4>` | Dry-run format (no writes, shows layout) |
| `mkswap` | `mkswap <drive> <partition>` | Format as swap |

> If `partition` is omitted, the **entire drive** is formatted.

### Examples

```bash
mkswap 1 1
mkfs.ext4 1 2
mkfs.ext4 3
```

---