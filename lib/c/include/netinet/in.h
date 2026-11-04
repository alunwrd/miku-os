/* IPv4 addresses. The IPv6 types are declared because code names them;
   nothing on MikuOS carries one */
#ifndef _NETINET_IN_H
#define _NETINET_IN_H

#include <_miku.h>
#include <sys/socket.h>
#include <bits/in.h>

typedef uint16_t in_port_t;
typedef uint32_t in_addr_t;

struct in_addr { in_addr_t s_addr; };
struct sockaddr_in {
    sa_family_t sin_family;
    in_port_t sin_port;
    struct in_addr sin_addr;
    unsigned char sin_zero[8];
};
struct in6_addr { unsigned char s6_addr[16]; };
struct sockaddr_in6 {
    sa_family_t sin6_family;
    in_port_t sin6_port;
    uint32_t sin6_flowinfo;
    struct in6_addr sin6_addr;
    uint32_t sin6_scope_id;
};

#ifndef INADDR_ANY
#define INADDR_ANY ((in_addr_t)0x00000000)
#endif
#ifndef INADDR_LOOPBACK
#define INADDR_LOOPBACK ((in_addr_t)0x7f000001)
#endif

static inline uint16_t htons(uint16_t x) { return __builtin_bswap16(x); }
static inline uint16_t ntohs(uint16_t x) { return __builtin_bswap16(x); }
static inline uint32_t htonl(uint32_t x) { return __builtin_bswap32(x); }
static inline uint32_t ntohl(uint32_t x) { return __builtin_bswap32(x); }

#endif
