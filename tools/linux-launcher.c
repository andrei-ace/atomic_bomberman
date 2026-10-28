#define _GNU_SOURCE
#define _XOPEN_SOURCE 700
#include <gtk/gtk.h>
#include <errno.h>
#include <fcntl.h>
#include <ftw.h>
#include <limits.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

// The launcher owns only its uniquely created staging directory. Publishing
// uses renameat2(RENAME_EXCHANGE), keeping a working cache across cancellation,
// failed validation, repair and crashes. Original data is always read-only.
static char *bin_dir, *data_dir, *cache, *state_dir, *stage, *replacement, *log_path;
static GPtrArray *game_args;
static GtkWidget *window, *status, *progress, *choose_file, *choose_folder, *cancel;
static GSubprocess *child;
static gboolean cancelled, import_only, close_after_cancel;
static int lock_fd = -1;
static volatile sig_atomic_t interrupted;

static void fail(const char *message) { fprintf(stderr, "Atomic Bomberman launcher: %s\n", message); exit(1); }
static int remove_entry(const char *path, const struct stat *info, int kind, struct FTW *walk) {
    (void)info; (void)kind; (void)walk; return remove(path);
}
static void cleanup(void) {
    if (stage) { nftw(stage, remove_entry, 32, FTW_DEPTH | FTW_PHYS); g_clear_pointer(&stage, g_free); }
    g_clear_pointer(&replacement, g_free);
}
static gboolean real_directory(const char *path) {
    struct stat info; return lstat(path, &info) == 0 && S_ISDIR(info.st_mode);
}
static void ensure_directory(const char *path) {
    if (g_mkdir_with_parents(path, 0700) < 0 || !real_directory(path)) fail("data location must be a writable real directory");
}
static char *xdg_path(const char *variable, const char *fallback) {
    const char *base = g_getenv(variable);
    char *path = base && *base ? g_strdup(base) : g_build_filename(g_get_home_dir(), fallback, NULL);
    char *result = g_build_filename(path, "atomic-bomberman", NULL); g_free(path); return result;
}
static void handle_signal(int number) { (void)number; interrupted = 1; }
static void child_setup(gpointer unused) { (void)unused; setpgid(0, 0); }
static void terminate_child(void) {
    if (child) {
        const char *id = g_subprocess_get_identifier(child);
        if (id) kill(-atoi(id), SIGTERM);
    }
}
static GSubprocess *spawn_helper(const char *command, const char *flag, const char *path, const char *output) {
    char *helper = g_build_filename(bin_dir, "bomberman-assets", NULL);
    GSubprocessLauncher *launcher = g_subprocess_launcher_new(G_SUBPROCESS_FLAGS_STDERR_MERGE);
    // Append so validation retains the preceding import diagnostics.
    int descriptor = open(log_path, O_WRONLY | O_CREAT | O_APPEND | O_NOFOLLOW, 0600);
    if (descriptor < 0) fail("cannot open private import log");
    g_subprocess_launcher_take_stdout_fd(launcher, descriptor);
    g_subprocess_launcher_set_child_setup(launcher, child_setup, NULL, NULL);
    GError *error = NULL;
    GSubprocess *process = output
        ? g_subprocess_launcher_spawn(launcher, &error, helper, command, flag, path, "--output", output, NULL)
        : g_subprocess_launcher_spawn(launcher, &error, helper, command, flag, path, NULL);
    g_object_unref(launcher); g_free(helper);
    if (error) { fprintf(stderr, "%s\n", error->message); g_error_free(error); }
    return process;
}
static gboolean wait_child(GSubprocess *process) {
    if (!process) return FALSE;
    return g_subprocess_wait_check(process, NULL, NULL);
}
static gboolean valid_cache(void) {
    if (!real_directory(cache)) return FALSE;
    GSubprocess *process = spawn_helper("validate", "--assets", cache, NULL);
    gboolean valid = wait_child(process);
    if (process) g_object_unref(process);
    return valid;
}
static gboolean publish(void) {
    struct stat info;
    if (lstat(cache, &info) == 0) {
        if (!S_ISDIR(info.st_mode)) return FALSE;
        if (syscall(SYS_renameat2, AT_FDCWD, replacement, AT_FDCWD, cache, 2 /* RENAME_EXCHANGE */) < 0) return FALSE;
    } else if (errno != ENOENT || rename(replacement, cache) < 0) return FALSE;
    int descriptor = open(data_dir, O_RDONLY | O_DIRECTORY);
    if (descriptor >= 0) { fsync(descriptor); close(descriptor); }
    return TRUE;
}
static void launch_game(void) {
    char *game = g_build_filename(bin_dir, "bomberman", NULL);
    g_ptr_array_insert(game_args, 0, game);
    g_ptr_array_add(game_args, "--assets"); g_ptr_array_add(game_args, cache); g_ptr_array_add(game_args, NULL);
    char *game_log = g_build_filename(state_dir, "game.log", NULL);
    int descriptor = open(game_log, O_WRONLY | O_CREAT | O_TRUNC | O_NOFOLLOW, 0600);
    if (descriptor >= 0) { dup2(descriptor, STDOUT_FILENO); dup2(descriptor, STDERR_FILENO); close(descriptor); }
    if (lock_fd >= 0) close(lock_fd);
    execv(game, (char **)game_args->pdata); fail("cannot start the bundled game");
}
static void finished(gboolean success) {
    cleanup();
    if (!window) {
        if (!success) fail(cancelled ? "import cancelled; previous cache preserved" : "import failed; previous cache preserved; see import.log");
        return;
    }
    g_clear_object(&child);
    gtk_widget_set_sensitive(choose_file, TRUE); gtk_widget_set_sensitive(choose_folder, TRUE);
    gtk_widget_set_sensitive(cancel, TRUE); gtk_button_set_label(GTK_BUTTON(cancel), "Quit");
    gtk_widget_hide(progress);
    if (close_after_cancel) { gtk_main_quit(); return; }
    if (success) { gtk_main_quit(); return; }
    gtk_label_set_text(GTK_LABEL(status), cancelled ? "Import cancelled. Your previous cache is preserved." : "Import failed. Choose another source to retry. See the import log.");
}
static gboolean pulse(gpointer unused) {
    (void)unused;
    if (interrupted && child) { cancelled = TRUE; terminate_child(); interrupted = 0; }
    if (child && progress) gtk_progress_bar_pulse(GTK_PROGRESS_BAR(progress));
    return G_SOURCE_CONTINUE;
}
static void validated(GObject *source, GAsyncResult *result, gpointer unused) {
    (void)unused;
    gboolean success = g_subprocess_wait_check_finish(G_SUBPROCESS(source), result, NULL);
    finished(success && !cancelled && publish());
}
static void imported(GObject *source, GAsyncResult *result, gpointer unused) {
    (void)unused;
    gboolean success = g_subprocess_wait_check_finish(G_SUBPROCESS(source), result, NULL);
    g_clear_object(&child);
    if (!success || cancelled) { finished(FALSE); return; }
    gtk_label_set_text(GTK_LABEL(status), "Validating imported game data...");
    child = spawn_helper("validate", "--assets", replacement, NULL);
    if (!child) { finished(FALSE); return; }
    g_subprocess_wait_check_async(child, NULL, validated, NULL);
}
static void prepare_import(const char *source_path) {
    (void)source_path;
    cleanup(); cancelled = FALSE;
    stage = g_build_filename(data_dir, ".import-XXXXXX", NULL);
    if (!g_mkdtemp(stage)) fail("cannot create import staging directory");
    replacement = g_build_filename(stage, "assets", NULL);
    int descriptor = open(log_path, O_WRONLY | O_CREAT | O_TRUNC | O_NOFOLLOW, 0600);
    if (descriptor < 0) fail("cannot open import log");
    close(descriptor);
}
static void choose_source(GtkButton *button, gpointer folder) {
    (void)button;
    GtkWidget *dialog = gtk_file_chooser_dialog_new("Choose Game Data", GTK_WINDOW(window),
        folder ? GTK_FILE_CHOOSER_ACTION_SELECT_FOLDER : GTK_FILE_CHOOSER_ACTION_OPEN,
        "Cancel", GTK_RESPONSE_CANCEL, "Import", GTK_RESPONSE_ACCEPT, NULL);
    if (gtk_dialog_run(GTK_DIALOG(dialog)) == GTK_RESPONSE_ACCEPT) {
        char *source = gtk_file_chooser_get_filename(GTK_FILE_CHOOSER(dialog));
        prepare_import(source);
        child = spawn_helper("import", "--source", source, replacement); g_free(source);
        gtk_widget_set_sensitive(choose_file, FALSE); gtk_widget_set_sensitive(choose_folder, FALSE);
        gtk_button_set_label(GTK_BUTTON(cancel), "Cancel"); gtk_widget_show(progress);
        gtk_label_set_text(GTK_LABEL(status), "Importing game data. This can take several minutes...");
        if (child) g_subprocess_wait_check_async(child, NULL, imported, NULL); else finished(FALSE);
    }
    gtk_widget_destroy(dialog);
}
static void cancel_job(GtkButton *button, gpointer unused) {
    (void)button; (void)unused;
    if (child) { cancelled = TRUE; terminate_child(); gtk_widget_set_sensitive(cancel, FALSE); }
    else { close_after_cancel = TRUE; gtk_main_quit(); }
}
static gboolean close_window(GtkWidget *widget, GdkEvent *event, gpointer unused) {
    (void)widget; (void)event; (void)unused; close_after_cancel = TRUE; cancel_job(NULL, NULL); return TRUE;
}
typedef struct { gboolean done, success; } CliWait;
static void cli_completed(GObject *source, GAsyncResult *result, gpointer context) {
    CliWait *wait = context;
    wait->success = g_subprocess_wait_check_finish(G_SUBPROCESS(source), result, NULL);
    wait->done = TRUE;
}
static gboolean wait_cancellable(GSubprocess *process) {
    if (!process) return FALSE;
    CliWait wait = {FALSE, FALSE};
    g_subprocess_wait_check_async(process, NULL, cli_completed, &wait);
    while (!wait.done) {
        if (interrupted) { cancelled = TRUE; terminate_child(); interrupted = 0; }
        g_main_context_iteration(NULL, FALSE); g_usleep(10000);
    }
    return wait.success;
}
static void import_cli(const char *source) {
    prepare_import(source);
    child = spawn_helper("import", "--source", source, replacement);
    if (!child) finished(FALSE);
    gboolean success = wait_cancellable(child);
    g_clear_object(&child);
    if (!success || cancelled) finished(FALSE);
    child = spawn_helper("validate", "--assets", replacement, NULL);
    success = wait_cancellable(child); g_clear_object(&child);
    finished(success && !cancelled && publish());
}
int main(int argc, char **argv) {
    signal(SIGTERM, handle_signal); signal(SIGINT, handle_signal);
    char executable[PATH_MAX]; ssize_t count = readlink("/proc/self/exe", executable, sizeof(executable)-1);
    if (count < 0) fail("cannot locate launcher");
    executable[count] = 0; bin_dir = g_path_get_dirname(executable);
    data_dir = xdg_path("XDG_DATA_HOME", ".local/share"); state_dir = xdg_path("XDG_STATE_HOME", ".local/state");
    game_args = g_ptr_array_new(); const char *source = NULL; gboolean repair = FALSE, explicit_assets = FALSE, help = FALSE;
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--import-source") || !strcmp(argv[i], "--data-directory")) {
            const char *option = argv[i]; if (++i == argc) fail("launcher option needs a path");
            if (!strcmp(option, "--import-source")) source = argv[i]; else { g_free(data_dir); data_dir = g_strdup(argv[i]); }
        } else if (!strcmp(argv[i], "--import-only")) import_only = TRUE;
        else if (!strcmp(argv[i], "--repair")) repair = TRUE;
        else { if (!strcmp(argv[i], "--assets")) explicit_assets = TRUE; if (!strcmp(argv[i], "--help") || !strcmp(argv[i], "-h")) help = TRUE; g_ptr_array_add(game_args, argv[i]); }
    }
    if (help || explicit_assets) {
        if (help) { puts("Linux launcher: --import-source <path> --import-only --data-directory <path> --repair\n"); fflush(stdout); }
        char *game = g_build_filename(bin_dir, "bomberman", NULL); g_ptr_array_insert(game_args, 0, game); g_ptr_array_add(game_args, NULL);
        execv(game, (char **)game_args->pdata); fail("cannot start bundled game");
    }
    ensure_directory(data_dir); ensure_directory(state_dir);
    cache = g_build_filename(data_dir, "assets", NULL); log_path = g_build_filename(state_dir, "import.log", NULL);
    char *lock_path = g_build_filename(data_dir, ".import.lock", NULL);
    lock_fd = open(lock_path, O_RDWR | O_CREAT | O_NOFOLLOW | O_CLOEXEC, 0600); g_free(lock_path);
    if (lock_fd < 0 || flock(lock_fd, LOCK_EX | LOCK_NB) < 0) fail("another launcher is importing game data");
    atexit(cleanup);
    if (source) import_cli(source);
    else if (repair || !valid_cache()) {
        if (!gtk_init_check(&argc, &argv)) fail("a desktop is required; use --import-source <path> --import-only for a terminal import");
        window = gtk_window_new(GTK_WINDOW_TOPLEVEL); gtk_window_set_title(GTK_WINDOW(window), "Atomic Bomberman");
        gtk_window_set_default_size(GTK_WINDOW(window), 600, 280); gtk_container_set_border_width(GTK_CONTAINER(window), 26);
        GtkWidget *box = gtk_box_new(GTK_ORIENTATION_VERTICAL, 16); gtk_container_add(GTK_CONTAINER(window), box);
        GtkWidget *title = gtk_label_new(NULL); gtk_label_set_markup(GTK_LABEL(title), "<span size='x-large' weight='bold'>Choose Game Data</span>");
        gtk_box_pack_start(GTK_BOX(box), title, FALSE, FALSE, 0);
        GtkWidget *description = gtk_label_new("Choose your original .7z archive, .img/.iso disc image, or extracted game folder.\nYour original files stay unchanged. Import is needed only once.");
        gtk_label_set_line_wrap(GTK_LABEL(description), TRUE); gtk_box_pack_start(GTK_BOX(box), description, FALSE, FALSE, 0);
        status = gtk_label_new((repair || g_file_test(cache, G_FILE_TEST_EXISTS)) ? "Repair the saved game data. Its validation failed or repair was requested." : "Welcome! Original game data is required to play.");
        gtk_label_set_line_wrap(GTK_LABEL(status), TRUE); gtk_box_pack_start(GTK_BOX(box), status, FALSE, FALSE, 0);
        progress = gtk_progress_bar_new(); gtk_box_pack_start(GTK_BOX(box), progress, FALSE, FALSE, 0);
        GtkWidget *buttons = gtk_box_new(GTK_ORIENTATION_HORIZONTAL, 10); gtk_box_pack_end(GTK_BOX(box), buttons, FALSE, FALSE, 0);
        choose_file = gtk_button_new_with_label("Choose Archive or Disc..."); choose_folder = gtk_button_new_with_label("Choose Folder..."); cancel = gtk_button_new_with_label("Quit");
        gtk_box_pack_start(GTK_BOX(buttons), choose_file, TRUE, TRUE, 0); gtk_box_pack_start(GTK_BOX(buttons), choose_folder, TRUE, TRUE, 0); gtk_box_pack_end(GTK_BOX(buttons), cancel, FALSE, FALSE, 0);
        g_signal_connect(choose_file, "clicked", G_CALLBACK(choose_source), NULL); g_signal_connect(choose_folder, "clicked", G_CALLBACK(choose_source), GINT_TO_POINTER(1));
        g_signal_connect(cancel, "clicked", G_CALLBACK(cancel_job), NULL); g_signal_connect(window, "delete-event", G_CALLBACK(close_window), NULL);
        gtk_widget_show_all(window); gtk_widget_hide(progress); g_timeout_add(100, pulse, NULL); gtk_main();
        if (!valid_cache() || close_after_cancel || cancelled) return 0;
    }
    if (import_only) { puts("Imported and validated game data."); return 0; }
    launch_game(); return 1;
}
