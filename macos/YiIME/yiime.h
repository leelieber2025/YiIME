// C interface of yiime-ffi (implemented in crates/yiime-ffi/src/lib.rs).
#include <stdbool.h>
#include <stdint.h>

typedef struct Ime Ime;
Ime *yiime_new(void);
void yiime_preload(void);
void yiime_free(Ime *ime);
bool yiime_key(Ime *ime, uint32_t ch);
bool yiime_insert_text(Ime *ime, const char *text);
void yiime_shift_tap(Ime *ime);
void yiime_commit_code(Ime *ime);
bool yiime_active(const Ime *ime);
char *yiime_state(const Ime *ime);
char *yiime_take_text(Ime *ime);
char *yiime_translate(const char *source, const char *url, const char *model, const char *key);
uint64_t yiime_begin_request(Ime *ime);
void yiime_cancel_request(Ime *ime);
bool yiime_finish_request(Ime *ime, uint64_t id);
void yiime_string_free(char *text);
