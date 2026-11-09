/* Address text: IPv4 only, as the stack is. AF_INET6 is EAFNOSUPPORT */
#ifndef _ARPA_INET_H
#define _ARPA_INET_H

#include <_miku.h>
#include <netinet/in.h>

__MIKU_BEGIN
int inet_pton(int, const char *, void *);
const char *inet_ntop(int, const void *, char *, socklen_t);
__MIKU_END

#endif
