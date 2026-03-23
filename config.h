#ifndef _WSK_CONFIG_H
#define _WSK_CONFIG_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

enum wsk_color_override { WSK_COLOR_DEFAULT, WSK_COLOR_NONE, WSK_COLOR_CUSTOM };
enum wsk_mod_override { WSK_MOD_DEFAULT, WSK_MOD_FORCE, WSK_MOD_SUPPRESS };

struct wsk_remap {
	char keysym[128];
	char display[128];
	bool has_display;
	enum wsk_color_override color_type;
	uint32_t custom_color;
	enum wsk_mod_override mod_override;
};

struct wsk_config {
	struct wsk_remap *entries;
	size_t count;
	size_t capacity;
};

/* Loads keymap.conf from $XDG_CONFIG_HOME/wshowkeys/ (or ~/.config fallback).
 * Returns 0 on success, -1 if no config found (not an error), 1 on parse error. */
int wsk_config_load(struct wsk_config *config);
void wsk_config_destroy(struct wsk_config *config);

/* Returns the remap entry for the given keysym name, or NULL if none. */
const struct wsk_remap *wsk_config_find(const struct wsk_config *config,
		const char *keysym);

uint32_t parse_color(const char *color);

#endif
