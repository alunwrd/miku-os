// The simplest GPU program: proves the chain of compute channel, graphics
// context, launch descriptor, program load and fence. If the buffer comes back
// holding value+i for every i, all of those worked.
//
// `bytes` is how much of `out` there is: a sized buffer (`PARAM_BUFFER_SIZED`,
// `dev/aosh-gpu.md`), so a `count` past it writes nothing past it.

extern "C" __global__ void fill(unsigned int *out, unsigned long long bytes, unsigned int value, unsigned int count) {
    unsigned int i = blockIdx.x * blockDim.x + threadIdx.x;
    if (i < count && (unsigned long long)i < bytes / 4) {
        out[i] = value + i;
    }
}
