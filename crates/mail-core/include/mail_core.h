#ifndef MAIL_CORE_H
#define MAIL_CORE_H
#ifdef __cplusplus
extern "C" {
#endif
/* Returns NULL if the database cannot be opened. Caller owns the store. */
void *mail_store_open(const char *path);
void mail_store_close(void *store);
/* Returns a JSON envelope: {"ok":true,"data":...} or {"ok":false,"error":"..."}.
   The returned string must be released with mail_store_free_string. */
char *mail_store_request_json(void *store, const char *request_json);
void mail_store_free_string(char *value);
#ifdef __cplusplus
}
#endif
#endif
