/* 独立公开向量与 provider 失败传播；来源见 THIRD_PARTY_NOTICES.md。
 * 显式链接本次 Cargo 构建的 SQLCipher / libcrypto，不能链接系统 OpenSSL。
 * 每种模式在独立进程启动；所有输入均为公开向量或合成数据。 */
#include <openssl/core_names.h>
#include <openssl/crypto.h>
#include <openssl/evp.h>
#include <openssl/params.h>
#include <openssl/rand.h>
#include <sqlite3.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define CHECK(condition) do { if (!(condition)) { \
    fprintf(stderr, "check failed at line %d: %s\n", __LINE__, #condition); \
    exit(1); \
} } while (0)

static size_t decode(const char *hex, unsigned char *out) {
    size_t length = strlen(hex) / 2;
    CHECK(strlen(hex) == length * 2);
    for (size_t i = 0; i < length; i++) {
        unsigned int byte;
        CHECK(sscanf(hex + i * 2, "%2x", &byte) == 1);
        out[i] = (unsigned char) byte;
    }
    return length;
}

static void aes_vector(void) {
    unsigned char key[32], iv[16], plain[64], cipher[64], actual[80], restored[80];
    decode("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4", key);
    decode("000102030405060708090a0b0c0d0e0f", iv);
    decode("6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51"
           "30c81c46a35ce411e5fbc1191a0a52eff69f2445df4f9b17ad2b417be66c3710", plain);
    decode("f58c4c04d6e5f1ba779eabfb5f7bfbd69cfc4e967edb808d679f777bc6702c7d"
           "39f23369a9d9bacfa530e26304231461b2eb05e2c39be9fcda6c19078c6a9d1b", cipher);
    EVP_CIPHER_CTX *ctx = EVP_CIPHER_CTX_new();
    CHECK(ctx != NULL);
    int length, tail;
    CHECK(EVP_EncryptInit_ex(ctx, EVP_aes_256_cbc(), NULL, key, iv) == 1);
    CHECK(EVP_CIPHER_CTX_set_padding(ctx, 0) == 1);
    CHECK(EVP_EncryptUpdate(ctx, actual, &length, plain, sizeof plain) == 1);
    CHECK(EVP_EncryptFinal_ex(ctx, actual + length, &tail) == 1);
    CHECK(length + tail == 64 && memcmp(actual, cipher, 64) == 0);
    CHECK(EVP_DecryptInit_ex(ctx, EVP_aes_256_cbc(), NULL, key, iv) == 1);
    CHECK(EVP_CIPHER_CTX_set_padding(ctx, 0) == 1);
    CHECK(EVP_DecryptUpdate(ctx, restored, &length, cipher, sizeof cipher) == 1);
    CHECK(EVP_DecryptFinal_ex(ctx, restored + length, &tail) == 1);
    CHECK(length + tail == 64 && memcmp(restored, plain, 64) == 0);
    EVP_CIPHER_CTX_free(ctx);
    puts("NIST SP800-38A F.2.5 AES-256-CBC encrypt/decrypt passed");
}

static void hmac_vector(void) {
    unsigned char key[20], actual[64], expected[64];
    memset(key, 0x0b, sizeof key);
    decode("87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cde"
           "daa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854", expected);
    EVP_MAC *mac = EVP_MAC_fetch(NULL, "HMAC", NULL);
    CHECK(mac != NULL);
    EVP_MAC_CTX *ctx = EVP_MAC_CTX_new(mac);
    CHECK(ctx != NULL);
    char digest[] = "SHA512";
    OSSL_PARAM params[] = {
        OSSL_PARAM_construct_utf8_string(OSSL_MAC_PARAM_DIGEST, digest, 0),
        OSSL_PARAM_construct_end()
    };
    size_t length;
    CHECK(EVP_MAC_init(ctx, key, sizeof key, params) == 1);
    CHECK(EVP_MAC_update(ctx, (const unsigned char *) "Hi There", 8) == 1);
    CHECK(EVP_MAC_final(ctx, actual, &length, sizeof actual) == 1);
    CHECK(length == 64 && memcmp(actual, expected, 64) == 0);
    EVP_MAC_CTX_free(ctx);
    EVP_MAC_free(mac);
    puts("RFC4231 section 4.2 HMAC-SHA512 passed");
}

static void pbkdf_vector(void) {
    unsigned char actual[64], expected[64];
    decode("55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc"
           "49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783", expected);
    CHECK(PKCS5_PBKDF2_HMAC("passwd", 6, (const unsigned char *) "salt", 4,
                           1, EVP_sha256(), 64, actual) == 1);
    CHECK(memcmp(actual, expected, 64) == 0);
    decode("4ddcd8f60b98be21830cee5ef22701f9641a4418d04c0414aeff08876b34ab56"
           "a1d425a1225833549adb841b51c9b3176a272bdebba1d078478f62b397f33c8d", expected);
    CHECK(PKCS5_PBKDF2_HMAC("Password", 8, (const unsigned char *) "NaCl", 4,
                           80000, EVP_sha256(), 64, actual) == 1);
    CHECK(memcmp(actual, expected, 64) == 0);
    puts("RFC7914 section 11 PBKDF2-HMAC-SHA256 c=1/80000 passed");
}

int main(int argc, char **argv) {
    CHECK(argc == 4);
    CHECK(strcmp(OpenSSL_version(OPENSSL_VERSION_STRING), argv[2]) == 0);
    int fail = strcmp(argv[1], "provider-failure") == 0;
    CHECK(fail || strcmp(argv[1], "vectors") == 0);
    CHECK(access(argv[3], F_OK) != 0);
    printf("linked OpenSSL=%s; mode=%s\n", OpenSSL_version(OPENSSL_VERSION), argv[1]);
    unsigned char random[32];
    if (fail) {
        CHECK(EVP_set_default_properties(NULL, "provider=radishledger_missing") == 1);
        CHECK(RAND_bytes(random, sizeof random) != 1);
    } else {
        aes_vector();
        hmac_vector();
        pbkdf_vector();
        CHECK(RAND_bytes(random, sizeof random) == 1);
    }
    sqlite3 *db = NULL;
    int result = sqlite3_open(argv[3], &db);
    unsigned char synthetic[32];
    memset(synthetic, 0x42, sizeof synthetic);
    if (result == SQLITE_OK) {
        result = sqlite3_key(db, synthetic, sizeof synthetic);
    }
    if (result == SQLITE_OK) {
        result = sqlite3_exec(db, "CREATE TABLE synthetic(value INTEGER);"
                             "INSERT INTO synthetic VALUES(42)", NULL, NULL, NULL);
    }
    CHECK(fail ? result != SQLITE_OK : result == SQLITE_OK);
    printf("SQLCipher keyed creation result=%d; expected %s\n", result, fail ? "failure" : "success");
    CHECK(sqlite3_close(db) == SQLITE_OK);
    if (access(argv[3], F_OK) == 0) {
        CHECK(unlink(argv[3]) == 0);
    }
    puts("no owned synthetic database remains");
    return 0;
}
