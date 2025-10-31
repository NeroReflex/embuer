/**
 * Embuer Status Monitor Example
 * 
 * This example demonstrates how to monitor update status changes in real-time
 * using the Embuer C library. It will continuously display status updates
 * until interrupted with Ctrl+C.
 * 
 * Compile with:
 *   gcc -o status_monitor status_monitor.c -L../target/release -lembuer -lpthread -ldl -lm
 * 
 * Run with:
 *   LD_LIBRARY_PATH=../target/release ./status_monitor
 */

#include <stdio.h>
#include <stdlib.h>
#include <signal.h>
#include <string.h>
#include <time.h>
#include "../embuer.h"

// Global flag for clean shutdown
static volatile int keep_running = 1;

/**
 * Signal handler for graceful shutdown
 */
void signal_handler(int signum) {
    printf("\n\nReceived signal %d, shutting down...\n", signum);
    keep_running = 0;
}

/**
 * Get current timestamp as a string
 */
void get_timestamp(char* buffer, size_t size) {
    time_t now = time(NULL);
    struct tm* tm_info = localtime(&now);
    strftime(buffer, size, "%Y-%m-%d %H:%M:%S", tm_info);
}

/**
 * Print a formatted status line
 */
void print_status(const char* status, const char* details, int progress) {
    char timestamp[64];
    get_timestamp(timestamp, sizeof(timestamp));
    
    printf("[%s] ", timestamp);
    
    // Color-code based on status
    if (strcmp(status, "Idle") == 0) {
        printf("\033[0;90m");  // Gray (bright black)
    } else if (strcmp(status, "Clearing") == 0) {
        printf("\033[0;36m");  // Cyan
    } else if (strcmp(status, "Installing") == 0) {
        printf("\033[0;33m");  // Yellow
    } else if (strcmp(status, "AwaitingConfirmation") == 0) {
        printf("\033[1;33m");  // Bold Yellow
    } else if (strcmp(status, "Failed") == 0) {
        printf("\033[0;31m");  // Red
    } else if (strcmp(status, "Completed") == 0) {
        printf("\033[0;32m");  // Green
    }
    
    printf("%-20s\033[0m", status);  // Reset color
    
    printf(" │ %-40s", details);
    
    if (progress >= 0) {
        // Draw progress bar
        printf(" │ [");
        int bar_width = 20;
        int filled = (progress * bar_width) / 100;
        for (int i = 0; i < bar_width; i++) {
            if (i < filled) {
                printf("█");
            } else {
                printf("░");
            }
        }
        printf("] %3d%%", progress);
    } else {
        printf(" │ %*s", 20 + 7, "N/A");  // Align with progress bar
    }
    
    printf("\n");
    fflush(stdout);
}

/**
 * Callback function called when status changes
 * 
 * This is invoked by the Embuer library whenever the update status changes
 */
void on_status_changed(
    const char* status,
    const char* details,
    int progress,
    void* user_data
) {
    // user_data is a pointer we passed to embuer_watch_status()
    // In this example, we're using it to count status updates
    int* update_count = (int*)user_data;
    (*update_count)++;
    
    print_status(status, details, progress);
    
    // Alert user when confirmation is required
    if (strcmp(status, "AwaitingConfirmation") == 0) {
        printf("\n");
        printf("\033[1;33m"); // Bold yellow
        printf("╔════════════════════════════════════════════════════════════════════════════╗\n");
        printf("║                        ⚠️  USER CONFIRMATION REQUIRED  ⚠️                  ║\n");
        printf("╠════════════════════════════════════════════════════════════════════════════╣\n");
        printf("║ An update is ready to install but requires your approval.                  ║\n");
        printf("║                                                                            ║\n");
        printf("║ To view the changelog and decide:                                          ║\n");
        printf("║   embuer-client pending-update    - View update details                    ║\n");
        printf("║   embuer-client accept            - Accept and install                     ║\n");
        printf("║   embuer-client reject            - Reject this update                     ║\n");
        printf("║                                                                            ║\n");
        printf("║ Or use the interactive update_confirmation tool:                           ║\n");
        printf("║   ./update_confirmation                                                    ║\n");
        printf("╚════════════════════════════════════════════════════════════════════════════╝\n");
        printf("\033[0m"); // Reset color
        printf("\n");
    }
}

/**
 * Display the initial status before starting monitoring
 */
int display_initial_status(embuer_client_t* client) {
    char* status = NULL;
    char* details = NULL;
    int progress = 0;
    
    int result = embuer_get_status(client, &status, &details, &progress);
    
    if (result == EMBUER_OK) {
        printf("\n");
        printf("┌─────────────────────────────────────────────────────────────────────────────┐\n");
        printf("│                     Embuer Update Status Monitor                            │\n");
        printf("├─────────────────────────────────────────────────────────────────────────────┤\n");
        printf("│ Press Ctrl+C to exit                                                        │\n");
        printf("└─────────────────────────────────────────────────────────────────────────────┘\n");
        printf("\n");
        printf("Current Status:\n");
        printf("───────────────\n");
        print_status(status, details, progress);
        printf("\n");
        printf("Monitoring for updates...\n");
        printf("────────────────────────────────────────────────────────────────────────────────\n");
        
        embuer_free_string(status);
        embuer_free_string(details);
        return 0;
    } else {
        fprintf(stderr, "Error getting initial status: ");
        switch (result) {
            case EMBUER_ERR_NULL_PTR:
                fprintf(stderr, "Null pointer\n");
                break;
            case EMBUER_ERR_CONNECTION:
                fprintf(stderr, "Connection error\n");
                break;
            case EMBUER_ERR_DBUS:
                fprintf(stderr, "D-Bus error (is the service running?)\n");
                break;
            case EMBUER_ERR_INVALID_STRING:
                fprintf(stderr, "Invalid string\n");
                break;
            case EMBUER_ERR_RUNTIME:
                fprintf(stderr, "Runtime error\n");
                break;
            default:
                fprintf(stderr, "Unknown error code: %d\n", result);
        }
        return -1;
    }
}

/**
 * Print usage statistics
 */
void print_statistics(int update_count, time_t start_time) {
    time_t end_time = time(NULL);
    double elapsed = difftime(end_time, start_time);
    
    printf("\n");
    printf("────────────────────────────────────────────────────────────────────────────────\n");
    printf("Session Statistics:\n");
    printf("  Duration:       %.0f seconds\n", elapsed);
    printf("  Updates seen:   %d\n", update_count);
    if (elapsed > 0) {
        printf("  Update rate:    %.2f updates/minute\n", (update_count * 60.0) / elapsed);
    }
    printf("────────────────────────────────────────────────────────────────────────────────\n");
}

int main(int argc, char* argv[]) {
    // Counter for status updates (passed as user_data to callback)
    int update_count = 0;
    time_t start_time = time(NULL);
    
    // Set up signal handlers for clean shutdown
    signal(SIGINT, signal_handler);   // Ctrl+C
    signal(SIGTERM, signal_handler);  // Termination signal
    
    // Create Embuer client
    printf("Connecting to Embuer service...\n");
    embuer_client_t* client = embuer_client_new();
    if (!client) {
        fprintf(stderr, "Failed to create Embuer client.\n");
        fprintf(stderr, "Make sure:\n");
        fprintf(stderr, "  1. The embuer-service is running (sudo embuer-service)\n");
        fprintf(stderr, "  2. D-Bus system bus is available\n");
        fprintf(stderr, "  3. You have permission to access the D-Bus service\n");
        return 1;
    }
    
    printf("Connected successfully!\n");
    
    // Display initial status
    if (display_initial_status(client) != 0) {
        embuer_client_free(client);
        return 1;
    }
    
    // Start watching for status changes
    // NOTE: embuer_watch_status() is a blocking call that will run until interrupted
    // The callback will be invoked whenever the status changes
    int result = embuer_watch_status(client, on_status_changed, &update_count);
    
    // This code is reached when embuer_watch_status() returns
    // (typically after the service stops or connection is lost)
    if (result != EMBUER_OK) {
        fprintf(stderr, "\nMonitoring stopped with error code: %d\n", result);
        if (result == EMBUER_ERR_DBUS) {
            fprintf(stderr, "The service may have stopped or the connection was lost.\n");
        }
    }
    
    // Print session statistics
    print_statistics(update_count, start_time);
    
    // Clean up
    embuer_client_free(client);
    
    printf("\nMonitoring session ended.\n");
    
    return 0;
}

