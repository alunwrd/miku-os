## /dev Virtual Filesystem

> Device files mounted at `/dev` via devfs.

| File | Description |
|---|---|
| `/dev/null` | Discards all writes, returns EOF on read |
| `/dev/zero` | Returns an infinite stream of zero bytes |
| `/dev/random` | Returns pseudo-random bytes |
| `/dev/urandom` | Returns pseudo-random bytes (non-blocking) |
| `/dev/console` | Writes go to the framebuffer console |

```bash
cat /dev/zero
cat /dev/random
write /dev/null garbage
```

---