/**
 * Embuer C Library
 * 
 * C interface for interacting with the Embuer update service.
 * 
 * Usage:
 * 1. Create a client with embuer_client_new()
 * 2. Use embuer_get_status(), embuer_install_from_file(), etc.
 * 3. Free strings with embuer_free_string()
 * 4. Free the client with embuer_client_free()
 * 
 * Example:
 * ```c
 * #include "embuer.h"
 * 
 * int main() {
 *     embuer_client_t* client = embuer_client_new();
 *     if (!client) {
 *         fprintf(stderr, "Failed to create client\n");
 *         return 1;
 *     }
 *     
 *     char* status = NULL;
 *     char* details = NULL;
 *     int progress = 0;
 *     
 *     int result = embuer_get_status(client, &status, &details, &progress);
 *     if (result == EMBUER_OK) {
 *         printf("Status: %s\n", status);
 *         printf("Details: %s\n", details);
 *         printf("Progress: %d%%\n", progress);
 *         
 *         embuer_free_string(status);
 *         embuer_free_string(details);
 *     }
 *     
 *     embuer_client_free(client);
 *     return 0;
 * }
 * ```
 */

#ifndef EMBUER_H
#define EMBUER_H

#ifdef __cplusplus
extern "C" {
#endif

#include <stdint.h>

/**
 * Opaque handle to the Embuer client
 */
typedef struct embuer_client_t embuer_client_t;

/**
 * Status callback function type
 * 
 * Parameters:
 * - status: Current status string
 * - details: Status details string
 * - progress: Progress value (0-100, or -1 if N/A)
 * - user_data: User-provided data pointer
 */
typedef void (*StatusCallback)(const char* status, const char* details, int progress, void* user_data);

/**
 * Error codes
 */
#define EMBUER_OK                0   /* Success */
#define EMBUER_ERR_NULL_PTR     -1   /* Null pointer passed */
#define EMBUER_ERR_CONNECTION   -2   /* Connection error */
#define EMBUER_ERR_DBUS         -3   /* D-Bus error */
#define EMBUER_ERR_INVALID_STRING -4 /* Invalid string encoding */
#define EMBUER_ERR_RUNTIME      -5   /* Runtime error */

/**
 * Initialize a new Embuer client
 * 
 * Returns:
 * - Pointer to client on success
 * - NULL on error
 */
embuer_client_t* embuer_client_new(void);

/**
 * Free an Embuer client
 * 
 * Parameters:
 * - client: Client handle to free
 */
void embuer_client_free(embuer_client_t* client);

/**
 * Get the current update status
 * 
 * Parameters:
 * - client: Client handle
 * - status_out: Pointer to receive status string (must be freed with embuer_free_string)
 * - details_out: Pointer to receive details string (must be freed with embuer_free_string)
 * - progress_out: Pointer to receive progress value (0-100, or -1 if N/A)
 * 
 * Returns:
 * - EMBUER_OK on success
 * - Error code on failure
 */
int embuer_get_status(
    embuer_client_t* client,
    char** status_out,
    char** details_out,
    int* progress_out
);

/**
 * Install an update from a file
 * 
 * Parameters:
 * - client: Client handle
 * - file_path: Path to the update file
 * - result_out: Pointer to receive result message (must be freed with embuer_free_string)
 * 
 * Returns:
 * - EMBUER_OK on success
 * - Error code on failure
 */
int embuer_install_from_file(
    embuer_client_t* client,
    const char* file_path,
    char** result_out
);

/**
 * Install an update from a URL
 * 
 * Parameters:
 * - client: Client handle
 * - url: URL to download the update from
 * - result_out: Pointer to receive result message (must be freed with embuer_free_string)
 * 
 * Returns:
 * - EMBUER_OK on success
 * - Error code on failure
 */
int embuer_install_from_url(
    embuer_client_t* client,
    const char* url,
    char** result_out
);

/**
 * Free a string allocated by the library
 * 
 * Parameters:
 * - s: String to free (can be NULL)
 */
void embuer_free_string(char* s);

/**
 * Watch for status updates (blocking call)
 * 
 * This function will block and call the callback whenever the status changes.
 * 
 * Parameters:
 * - client: Client handle
 * - callback: Function to call on status updates
 * - user_data: User data to pass to the callback (can be NULL)
 * 
 * Returns:
 * - EMBUER_OK on success
 * - Error code on failure
 */
int embuer_watch_status(
    embuer_client_t* client,
    StatusCallback callback,
    void* user_data
);

#ifdef __cplusplus
}
#endif

#endif /* EMBUER_H */

