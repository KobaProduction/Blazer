#ifndef BLAZER_SELECTOR_CALLBACKS_H
#define BLAZER_SELECTOR_CALLBACKS_H
#include <limits.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

extern uint64_t blazecore_get_capabilities(void) __attribute__((weak_import));
extern int blazecore_select_outbound(long, const char *, const char *) __attribute__((weak_import));

static inline uint64_t blazer_core_selector_capabilities(void) {
    return blazecore_get_capabilities ? blazecore_get_capabilities() : 0;
}

static inline int32_t blazer_core_selector_ready(uint64_t generation) {
    if (!(blazer_core_selector_capabilities() & UINT64_C(1))) return -6;
    if (!generation || generation > LONG_MAX) return -1;
    return blazecore_is_running() ? 0 : -2;
}

static inline int32_t blazer_core_selector_select(
    uint64_t generation, const uint8_t *group, size_t group_length,
    const uint8_t *member, size_t member_length
) {
    if (!(blazer_core_selector_capabilities() & UINT64_C(1)) || !blazecore_select_outbound) return -6;
    if (!generation || generation > LONG_MAX || !group || !member
        || !group_length || group_length > 127 || !member_length || member_length > 127) return -1;
    char group_name[128], member_name[128];
    memcpy(group_name, group, group_length);
    memcpy(member_name, member, member_length);
    group_name[group_length] = 0;
    member_name[member_length] = 0;
    return blazecore_select_outbound((long)generation, group_name, member_name);
}
#endif
