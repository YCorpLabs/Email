#ifndef MAIL_SERVICE_H
#define MAIL_SERVICE_H
#ifdef __cplusplus
extern "C" {
#endif
/* Open a local cache and account settings. Returns NULL on failure. */
void *mail_service_open(const char *path);
void mail_service_close(void *service);
/* JSON envelope: {"ok":true,"data":...} or {"ok":false,"error":"..."}. */
char *mail_service_request_json(void *service, const char *request_json);
void mail_service_free_string(char *value);
/* The build's distribution profile as JSON (no credentials). Free the result
   with mail_service_free_string. */
char *mail_service_distribution_profile_json(void);
#ifdef __cplusplus
}
#endif
#endif
