/**
 * Example C program using the Embuer library
 * 
 * Compile with:
 *   gcc -o embuer_example embuer_example.c -L../target/release -lembuer -lpthread -ldl -lm
 * 
 * Run with:
 *   LD_LIBRARY_PATH=../target/release ./embuer_example
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "../embuer.h"

void status_callback(const char* status, const char* details, int progress, void* user_data) {
    printf("[Callback] Status: %s\n", status);
    printf("[Callback] Details: %s\n", details);
    if (progress >= 0) {
        printf("[Callback] Progress: %d%%\n", progress);
    } else {
        printf("[Callback] Progress: N/A\n");
    }
    printf("\n");
}

void print_current_status(embuer_client_t* client) {
    char* status = NULL;
    char* details = NULL;
    int progress = 0;
    
    printf("Getting current status...\n");
    int result = embuer_get_status(client, &status, &details, &progress);
    
    if (result == EMBUER_OK) {
        printf("Status: %s\n", status);
        printf("Details: %s\n", details);
        if (progress >= 0) {
            printf("Progress: %d%%\n", progress);
        } else {
            printf("Progress: N/A\n");
        }
        
        embuer_free_string(status);
        embuer_free_string(details);
    } else {
        fprintf(stderr, "Failed to get status. Error code: %d\n", result);
    }
    printf("\n");
}

void install_from_file_example(embuer_client_t* client, const char* path) {
    char* result = NULL;
    
    printf("Installing update from file: %s\n", path);
    int code = embuer_install_from_file(client, path, &result);
    
    if (code == EMBUER_OK) {
        printf("Result: %s\n", result);
        embuer_free_string(result);
    } else {
        fprintf(stderr, "Failed to install from file. Error code: %d\n", code);
    }
    printf("\n");
}

void install_from_url_example(embuer_client_t* client, const char* url) {
    char* result = NULL;
    
    printf("Installing update from URL: %s\n", url);
    int code = embuer_install_from_url(client, url, &result);
    
    if (code == EMBUER_OK) {
        printf("Result: %s\n", result);
        embuer_free_string(result);
    } else {
        fprintf(stderr, "Failed to install from URL. Error code: %d\n", code);
    }
    printf("\n");
}

int main(int argc, char* argv[]) {
    printf("Embuer C Library Example\n");
    printf("========================\n\n");
    
    // Create client
    printf("Creating Embuer client...\n");
    embuer_client_t* client = embuer_client_new();
    if (!client) {
        fprintf(stderr, "Failed to create Embuer client\n");
        return 1;
    }
    printf("Client created successfully\n\n");
    
    // Print current status
    print_current_status(client);
    
    // Example: Install from file (if path provided)
    if (argc > 1 && strcmp(argv[1], "--install-file") == 0 && argc > 2) {
        install_from_file_example(client, argv[2]);
        print_current_status(client);
    }
    
    // Example: Install from URL (if URL provided)
    if (argc > 1 && strcmp(argv[1], "--install-url") == 0 && argc > 2) {
        install_from_url_example(client, argv[2]);
        print_current_status(client);
    }
    
    // Example: Watch for status updates (if --watch provided)
    if (argc > 1 && strcmp(argv[1], "--watch") == 0) {
        printf("Watching for status updates (press Ctrl+C to exit)...\n\n");
        embuer_watch_status(client, status_callback, NULL);
    }
    
    // Clean up
    printf("Cleaning up...\n");
    embuer_client_free(client);
    printf("Done\n");
    
    return 0;
}

