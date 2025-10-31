/**
 * Embuer Update Confirmation Example
 * 
 * This example demonstrates how to handle update confirmations when
 * auto_install_updates is disabled. It monitors for pending updates
 * and allows the user to view changelog and accept/reject them.
 * 
 * Compile with:
 *   gcc -o update_confirmation update_confirmation.c -L../target/release -lembuer -lpthread -ldl -lm
 * 
 * Run with:
 *   LD_LIBRARY_PATH=../target/release ./update_confirmation
 */

#include <stdio.h>
#include <stdlib.h>
#include <signal.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <ctype.h>
#include "../embuer.h"

// Global flag for clean shutdown
static volatile int keep_running = 1;
static volatile int pending_update_detected = 0;

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
 * Display a formatted box with text
 */
void print_box_header(const char* title) {
    printf("\n╔════════════════════════════════════════════════════════════════════════════╗\n");
    printf("║ %-74s ║\n", title);
    printf("╠════════════════════════════════════════════════════════════════════════════╣\n");
}

void print_box_line(const char* text) {
    printf("║ %-74s ║\n", text);
}

void print_box_footer(void) {
    printf("╚════════════════════════════════════════════════════════════════════════════╝\n");
}

/**
 * Display the pending update details with formatted output
 */
int display_pending_update(embuer_client_t* client) {
    char* version = NULL;
    char* changelog = NULL;
    char* source = NULL;
    
    int result = embuer_get_pending_update(client, &version, &changelog, &source);
    
    if (result != EMBUER_OK) {
        if (result == EMBUER_ERR_NO_PENDING_UPDATE) {
            fprintf(stderr, "No pending update awaiting confirmation.\n");
        } else {
            fprintf(stderr, "Error getting pending update: %d\n", result);
        }
        return -1;
    }
    
    print_box_header("PENDING UPDATE AWAITING CONFIRMATION");
    
    char info_line[128];
    snprintf(info_line, sizeof(info_line), "Version: %s", version);
    print_box_line(info_line);
    
    snprintf(info_line, sizeof(info_line), "Source:  %s", source);
    print_box_line(info_line);
    
    printf("╠════════════════════════════════════════════════════════════════════════════╣\n");
    print_box_line("CHANGELOG");
    printf("╠════════════════════════════════════════════════════════════════════════════╣\n");
    
    // Print changelog line by line
    char* line = strtok(changelog, "\n");
    while (line != NULL) {
        print_box_line(line);
        line = strtok(NULL, "\n");
    }
    
    printf("╠════════════════════════════════════════════════════════════════════════════╣\n");
    print_box_line("Commands:");
    print_box_line("  y / yes    - Accept and install the update");
    print_box_line("  n / no     - Reject the update");
    print_box_line("  q / quit   - Exit without deciding");
    print_box_footer();
    
    embuer_free_string(version);
    embuer_free_string(changelog);
    embuer_free_string(source);
    
    return 0;
}

/**
 * Prompt user for confirmation and handle the response
 */
int handle_user_decision(embuer_client_t* client) {
    char input[64];
    char* result_msg = NULL;
    
    while (1) {
        printf("\nYour decision [y/n/q]: ");
        fflush(stdout);
        
        if (fgets(input, sizeof(input), stdin) == NULL) {
            break;
        }
        
        // Remove trailing newline
        input[strcspn(input, "\n")] = 0;
        
        // Convert to lowercase for easier comparison
        for (char* p = input; *p; p++) {
            *p = tolower(*p);
        }
        
        if (strcmp(input, "y") == 0 || strcmp(input, "yes") == 0) {
            printf("\n✓ Accepting update...\n");
            
            int result = embuer_confirm_update(client, 1, &result_msg);
            if (result == EMBUER_OK) {
                printf("✓ %s\n", result_msg);
                printf("  Monitoring installation progress...\n\n");
                embuer_free_string(result_msg);
                return 0;
            } else {
                fprintf(stderr, "✗ Failed to accept update: %d\n", result);
                return -1;
            }
        } else if (strcmp(input, "n") == 0 || strcmp(input, "no") == 0) {
            printf("\n✗ Rejecting update...\n");
            
            int result = embuer_confirm_update(client, 0, &result_msg);
            if (result == EMBUER_OK) {
                printf("✗ %s\n", result_msg);
                embuer_free_string(result_msg);
                return 0;
            } else {
                fprintf(stderr, "✗ Failed to reject update: %d\n", result);
                return -1;
            }
        } else if (strcmp(input, "q") == 0 || strcmp(input, "quit") == 0) {
            printf("\nExiting without deciding...\n");
            return 1;
        } else {
            printf("Invalid input. Please enter 'y', 'n', or 'q'.\n");
        }
    }
    
    return -1;
}

/**
 * Callback function called when status changes
 */
void on_status_changed(
    const char* status,
    const char* details,
    int progress,
    void* user_data
) {
    char timestamp[64];
    get_timestamp(timestamp, sizeof(timestamp));
    
    // Check if we've entered AwaitingConfirmation state
    if (strcmp(status, "AwaitingConfirmation") == 0) {
        pending_update_detected = 1;
        printf("\n[%s] 🔔 UPDATE AVAILABLE - User confirmation required!\n", timestamp);
        printf("Press Ctrl+C to review and decide...\n\n");
        return;
    }
    
    // Print status update with formatting
    printf("[%s] ", timestamp);
    
    // Color-code based on status
    if (strcmp(status, "Idle") == 0) {
        printf("\033[0;90m");  // Gray (bright black)
    } else if (strcmp(status, "Clearing") == 0) {
        printf("\033[0;36m");  // Cyan
    } else if (strcmp(status, "Installing") == 0) {
        printf("\033[0;33m");  // Yellow
    } else if (strcmp(status, "Failed") == 0) {
        printf("\033[0;31m");  // Red
    } else if (strcmp(status, "Completed") == 0) {
        printf("\033[0;32m");  // Green
    }
    
    printf("%-20s\033[0m", status);  // Reset color
    
    if (strlen(details) > 0) {
        printf(" │ %-40s", details);
    }
    
    if (progress >= 0) {
        printf(" │ %3d%%", progress);
    }
    
    printf("\n");
    fflush(stdout);
}

/**
 * Main program loop
 */
int main(int argc, char* argv[]) {
    // Set up signal handlers for clean shutdown
    signal(SIGINT, signal_handler);
    signal(SIGTERM, signal_handler);
    
    // Create Embuer client
    printf("Connecting to Embuer service...\n");
    embuer_client_t* client = embuer_client_new();
    if (!client) {
        fprintf(stderr, "Failed to create Embuer client.\n");
        fprintf(stderr, "Make sure:\n");
        fprintf(stderr, "  1. The embuer-service is running\n");
        fprintf(stderr, "  2. D-Bus system bus is available\n");
        fprintf(stderr, "  3. auto_install_updates is set to false in config\n");
        return 1;
    }
    
    printf("Connected successfully!\n");
    
    // Check for pending update first
    char* version = NULL;
    char* changelog = NULL;
    char* source = NULL;
    
    int pending_check = embuer_get_pending_update(client, &version, &changelog, &source);
    if (pending_check == EMBUER_OK) {
        printf("\n⚠️  There is already a pending update!\n");
        embuer_free_string(version);
        embuer_free_string(changelog);
        embuer_free_string(source);
        
        if (display_pending_update(client) == 0) {
            int decision = handle_user_decision(client);
            if (decision != 0) {
                embuer_client_free(client);
                return decision;
            }
        }
    }
    
    // Print header
    print_box_header("UPDATE CONFIRMATION MONITOR");
    print_box_line("Waiting for updates to become available...");
    print_box_line("When an update requires confirmation, you will be prompted.");
    print_box_line("Press Ctrl+C to exit.");
    print_box_footer();
    
    // Start monitoring in background (this is a blocking call normally,
    // but we'll handle it differently by checking status periodically)
    
    // Alternatively, use a simpler polling approach
    printf("\nMonitoring for updates...\n");
    
    while (keep_running) {
        char* status = NULL;
        char* details = NULL;
        int progress = 0;
        
        int result = embuer_get_status(client, &status, &details, &progress);
        
        if (result == EMBUER_OK) {
            if (strcmp(status, "AwaitingConfirmation") == 0 && !pending_update_detected) {
                pending_update_detected = 1;
                
                printf("\n\n🔔 UPDATE AVAILABLE - Confirmation required!\n");
                embuer_free_string(status);
                embuer_free_string(details);
                
                if (display_pending_update(client) == 0) {
                    int decision = handle_user_decision(client);
                    if (decision != 0) {
                        embuer_client_free(client);
                        return decision;
                    }
                    
                    // Reset flag after handling
                    pending_update_detected = 0;
                }
            } else if (strcmp(status, "AwaitingConfirmation") != 0) {
                pending_update_detected = 0;
            }
            
            embuer_free_string(status);
            embuer_free_string(details);
        }
        
        sleep(2);  // Poll every 2 seconds
    }
    
    // Clean up
    embuer_client_free(client);
    
    printf("\nMonitor stopped.\n");
    
    return 0;
}

