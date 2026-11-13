// Synthetic-only fixture. Never searches the user's default keychains.
#include <Security/Security.h>
#include <stdio.h>
#include <string.h>
#ifndef BUILD_LABEL
#define BUILD_LABEL "A"
#endif
static const char password[] = "synthetic-keychain-password";
static const char value[] = "synthetic-secret-not-a-provider-token";
static const char *accounts[] = {"mail", "calendar", "booking"};
static OSStatus put(SecKeychainRef keychain, const char *service, const char *account) {
    return SecKeychainAddGenericPassword(keychain, (UInt32)strlen(service), service,
        (UInt32)strlen(account), account, sizeof(value)-1, value, NULL);
}
static OSStatus read_item(SecKeychainRef keychain, const char *service, const char *account) {
    UInt32 length=0; void *bytes=NULL;
    OSStatus status=SecKeychainFindGenericPassword(keychain, (UInt32)strlen(service), service,
        (UInt32)strlen(account), account, &length, &bytes, NULL);
    if (status == errSecSuccess) {
        if (length != sizeof(value)-1 || memcmp(bytes, value, length)) status=errSecDecode;
        SecKeychainItemFreeContent(NULL, bytes);
    }
    return status;
}
int main(int argc, char **argv) {
    if (argc!=3) return 64;
    SecKeychainSetUserInteractionAllowed(false);
    SecKeychainRef keychain=NULL;
    OSStatus status;
    if (!strcmp(argv[1], "create") || !strcmp(argv[1], "create-v3")) {
        status=SecKeychainCreate(argv[2], sizeof(password)-1, password, false, NULL, &keychain);
    } else status=SecKeychainOpen(argv[2], &keychain);
    if (status) goto done;
    if (!strcmp(argv[1], "create")) {
        status=SecKeychainUnlock(keychain,sizeof(password)-1,password,true);

        for (int i=0; !status && i<3; ++i) status=put(keychain,"email.synthetic.v2",accounts[i]);
    } else if (!strcmp(argv[1], "create-v3")) {
        status=SecKeychainUnlock(keychain,sizeof(password)-1,password,true);
        // Explicitly partitioned by the harness after creation. No real items.
        for (int i=0; !status && i<3; ++i) status=put(keychain,"email.synthetic.v3",accounts[i]);
    } else if (!strcmp(argv[1], "adopt-v4")) {
        // Models the destination creation AFTER an OS-approved historical read.
        // The value is a public synthetic fixture constant, not a real token.
        for (int i=0; !status && i<3; ++i) status=put(keychain,"email.synthetic.v4",accounts[i]);
    } else if (!strcmp(argv[1], "read-v4")) {
        for (int i=0; !status && i<3; ++i) status=read_item(keychain,"email.synthetic.v4",accounts[i]);
    } else if (!strcmp(argv[1], "read-v3")) {
        for (int i=0; !status && i<3; ++i) status=read_item(keychain,"email.synthetic.v3",accounts[i]);
    } else if (!strcmp(argv[1], "migrate")) {
        for (int i=0; !status && i<3; ++i) {
            status=read_item(keychain,"email.synthetic.v2",accounts[i]);
            if (!status) status=put(keychain,"email.synthetic.v3",accounts[i]);
        }
    } else if (!strcmp(argv[1], "read")) {
        for (int i=0; !status && i<3; ++i) status=read_item(keychain,"email.synthetic.v3",accounts[i]);
    } else if (!strcmp(argv[1], "read-old")) {
        for (int i=0; !status && i<3; ++i) status=read_item(keychain,"email.synthetic.v2",accounts[i]);
    } else if (!strcmp(argv[1], "lock")) status=SecKeychainLock(keychain);
    else if (!strcmp(argv[1], "unlock")) status=SecKeychainUnlock(keychain,sizeof(password)-1,password,true);
    else status=errSecParam;
done:
    if(keychain) CFRelease(keychain);
    // Status and fixture version only. Never print a secret.
    printf("fixture-%s %s status=%d\n",BUILD_LABEL,argv[1],(int)status);
    return status ? 1 : 0;
}
