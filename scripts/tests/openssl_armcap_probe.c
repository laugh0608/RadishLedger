/* 编译实际 OpenSSL armcap.c，以合成 auxv 和探测计数验证能力选择。
 * 只用于诊断，不进入 App；真实目标验证不得设置 OPENSSL_armcap。
 * 编译方法与独立期望来源见 scripts/README.md。 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/auxv.h>
#include "arm_arch.h"

extern unsigned int OPENSSL_armcap_P;
static int vector_calls;

static const char *input(void) {
    const char *bits = getenv("TEST_CAPS");
    if (bits == NULL || strlen(bits) != 2
        || (bits[0] != '0' && bits[0] != '1')
        || (bits[1] != '0' && bits[1] != '1')) {
        fputs("TEST_CAPS must be 00, 01, 10 or 11\n", stderr);
        exit(2);
    }
    return bits;
}

unsigned long probe_getauxval(unsigned long key) {
    const char *bits = input();
    /* NEON, AES, PMULL, SHA1, SHA256 在所有组合中都应保留。 */
    unsigned long base = (1UL << 1) | (1UL << 3) | (1UL << 4)
        | (1UL << 5) | (1UL << 6);
    if (key == AT_HWCAP)
        return base | (bits[0] == '1' ? 1UL << 22 : 0);
    if (key == AT_HWCAP2)
        return bits[1] == '1' ? 1UL << 1 : 0;
    return 0;
}

uint32_t _armv7_tick(void) { return 0; }
unsigned int _armv8_cpuid_probe(void) { return 0; }
size_t OPENSSL_rndr_asm(unsigned char *p, size_t n) {
    (void) p;
    (void) n;
    abort();
}
size_t OPENSSL_rndrrs_asm(unsigned char *p, size_t n) {
    (void) p;
    (void) n;
    abort();
}
uint64_t _armv8_sve_get_vl_bytes(void) {
    vector_calls++;
    return 32;
}

int main(void) {
    const char *bits = input();
    const unsigned int base = ARMV7_NEON | ARMV8_AES | ARMV8_PMULL
        | ARMV8_SHA1 | ARMV8_SHA256;
    /* 独立表：无 SVE 时不能选 SVE2；有效 SVE2 应保留探测。 */
    const unsigned int extra[] = {
        0, 0, ARMV8_SVE, ARMV8_SVE | ARMV9_SVE2 | ARMV9_SVE2_POLY1305
    };
    const int calls[] = { 0, 0, 0, 1 };
    unsigned int index = (unsigned int) ((bits[0] - '0') * 2 + bits[1] - '0');
    unsigned int expected = base | extra[index];
    printf("input=%s armcap=%x expected=%x vector_calls=%d expected_calls=%d\n",
           bits, OPENSSL_armcap_P, expected, vector_calls, calls[index]);
    return OPENSSL_armcap_P == expected && vector_calls == calls[index] ? 0 : 1;
}
