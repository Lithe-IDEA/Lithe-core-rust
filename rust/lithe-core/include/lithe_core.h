#ifndef LITHE_CORE_PUBLIC_H
#define LITHE_CORE_PUBLIC_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

const char *lithe_core_version(void);
/* Borrowed UTF-8 slice: 1 text, 0 binary controls, -1 invalid input. */
int32_t lithe_core_is_plain_text(const uint8_t *bytes, size_t length);
int32_t lithe_core_git_askpass(const char *prompt);
char *lithe_core_execute_json(const char *request);
/* Immutable source borrowed only until return; response uses lithe_core_free_string. */
char *lithe_core_execute_document_json(const char *request, const uint8_t *bytes, size_t length);
char *lithe_core_execute_json_with_events(const char *request, void (*callback)(const char *, void *), void *context);
char *lithe_core_lsp_provider_catalog_json(const char *workspace_root);
int32_t lithe_core_cancel(const char *operation_id);
void lithe_core_free_string(char *value);
void lithe_core_install_log_waker(void (*callback)(void *), void *context);
int32_t lithe_core_remove_log_waker(void *context);

#ifdef __cplusplus
}
#endif

#endif
