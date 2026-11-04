/* No include guard: assert.h is re-read with a different NDEBUG */
#include <_miku.h>

#undef assert
#ifdef NDEBUG
#define assert(e) ((void)0)
#else
__MIKU_BEGIN
__miku_noreturn void __assert_fail(const char *expr, const char *file, unsigned line, const char *func);
__MIKU_END
#define assert(e) ((e) ? (void)0 : __assert_fail(#e, __FILE__, __LINE__, __func__))
#endif

#ifndef __cplusplus
#undef static_assert
#define static_assert _Static_assert
#endif
