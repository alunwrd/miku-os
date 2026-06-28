## Scheduler

| Parameter | Value |
|---|---|
| Type | CFS (Completely Fair Scheduler), preemptive |
| Max processes | 4096 |
| Stack per process | 512 KB |
| Timer frequency | 250 Hz (PIT) |
| CPU window | 250 ticks (1 second) |

### Process States

| State | Description |
|---|---|
| `Ready` | Waiting to be scheduled |
| `Running` | Currently executing |
| `Sleeping` | Waiting for N ticks to pass |
| `Blocked` | Blocked on a resource |
| `Dead` | Finished, slot can be reused |

### Implementation Notes

Context switch is implemented in naked asm. `schedule_from_isr` acquires zero mutexes - the ISR uses atomics only.

### Scheduler API

```rust
scheduler::spawn(my_task);
scheduler::yield_now();
scheduler::sleep(ticks);
scheduler::kill(pid);
scheduler::current_pid() -> u64;
```

---