#ifndef RADISHLEDGER_PROBE_H
#define RADISHLEDGER_PROBE_H
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* M0 probe ABI v1。输入为调用期间借用的 UTF-8；handle 不是指针。 */
uint32_t radishledger_probe_abi(void);
uint64_t radishledger_probe_run(const uint8_t *input, size_t length);
uint64_t radishledger_probe_library_path(void);
/* M0 故障实验 JSON；与 run 相同的 UTF-8 长度 / 结果所有权约束。 */
uint64_t radishledger_probe_validate(const uint8_t *input, size_t length);
int32_t radishledger_probe_buffer_count(void);
size_t radishledger_probe_buffer_len(uint64_t handle);
/* 容量不足不写入，返回 0；成功返回完整长度。 */
size_t radishledger_probe_buffer_copy(uint64_t handle, uint8_t *output, size_t capacity);
/* 成功释放返回 1；未知 / 已释放 handle 返回 0。 */
int32_t radishledger_probe_buffer_free(uint64_t handle);

#ifdef __cplusplus
}
#endif
#endif
