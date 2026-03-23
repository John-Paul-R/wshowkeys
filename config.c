#include <ctype.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "config.h"

#define CONFIG_DIR "wshowkeys"
#define CONFIG_FILE "keymap.conf"

uint32_t parse_color(const char *color) {
	if (color[0] == '#') {
		++color;
	}

	int len = strlen(color);
	if (len != 6 && len != 8) {
		fprintf(stderr, "Invalid color %s, defaulting to color "
				"0xFFFFFFFF\n", color);
		return 0xFFFFFFFF;
	}
	uint32_t res = (uint32_t)strtoul(color, NULL, 16);
	if (len == 6) {
		res = (res << 8) | 0xFF;
	}
	return res;
}

static char *trim(char *s) {
	while (isspace((unsigned char)*s)) s++;
	if (*s == '\0') return s;
	char *end = s + strlen(s) - 1;
	while (end > s && isspace((unsigned char)*end)) end--;
	end[1] = '\0';
	return s;
}

static struct wsk_remap *config_get_or_create(struct wsk_config *config,
		const char *keysym) {
	for (size_t i = 0; i < config->count; i++) {
		if (strcmp(config->entries[i].keysym, keysym) == 0) {
			return &config->entries[i];
		}
	}

	if (config->count == config->capacity) {
		size_t new_cap = config->capacity ? config->capacity * 2 : 16;
		struct wsk_remap *new_entries = realloc(config->entries,
				new_cap * sizeof(struct wsk_remap));
		if (!new_entries) return NULL;
		config->entries = new_entries;
		config->capacity = new_cap;
	}

	struct wsk_remap *entry = &config->entries[config->count++];
	memset(entry, 0, sizeof(*entry));
	snprintf(entry->keysym, sizeof(entry->keysym), "%s", keysym);
	return entry;
}

/* Parse the :fmt value: "<color>[,m|,!m]" */
static int parse_fmt(struct wsk_remap *entry, const char *value, int lineno) {
	char buf[256];
	snprintf(buf, sizeof(buf), "%s", value);

	char *comma = strchr(buf, ',');
	char *mod_str = NULL;
	if (comma) {
		*comma = '\0';
		mod_str = trim(comma + 1);
	}
	char *color_str = trim(buf);

	if (strlen(color_str) > 0) {
		if (strcmp(color_str, "default") == 0) {
			entry->color_type = WSK_COLOR_DEFAULT;
		} else if (strcmp(color_str, "none") == 0) {
			entry->color_type = WSK_COLOR_NONE;
		} else if (color_str[0] == '#') {
			entry->color_type = WSK_COLOR_CUSTOM;
			entry->custom_color = parse_color(color_str);
		} else {
			fprintf(stderr, "keymap.conf:%d: invalid color '%s'\n",
					lineno, color_str);
			return 1;
		}
	}

	if (mod_str && strlen(mod_str) > 0) {
		if (strcmp(mod_str, "m") == 0) {
			entry->mod_override = WSK_MOD_FORCE;
		} else if (strcmp(mod_str, "!m") == 0) {
			entry->mod_override = WSK_MOD_SUPPRESS;
		} else {
			fprintf(stderr, "keymap.conf:%d: invalid modifier '%s'"
					" (expected 'm' or '!m')\n", lineno, mod_str);
			return 1;
		}
	}

	return 0;
}

static int parse_line(struct wsk_config *config, char *line, int lineno) {
	char *trimmed = trim(line);
	if (trimmed[0] == '\0' || trimmed[0] == '#') return 0;

	char *eq = strchr(trimmed, '=');
	if (!eq) {
		fprintf(stderr, "keymap.conf:%d: missing '='\n", lineno);
		return 1;
	}

	*eq = '\0';
	char *lhs = trim(trimmed);
	char *rhs = trim(eq + 1);

	/* Check for :fmt suffix */
	char *fmt_sep = strstr(lhs, ":fmt");
	if (fmt_sep) {
		*fmt_sep = '\0';
		char *keysym = trim(lhs);
		struct wsk_remap *entry = config_get_or_create(config, keysym);
		if (!entry) return 1;
		return parse_fmt(entry, rhs, lineno);
	}

	/* Display remap */
	char *keysym = lhs;
	struct wsk_remap *entry = config_get_or_create(config, keysym);
	if (!entry) return 1;
	entry->has_display = true;
	snprintf(entry->display, sizeof(entry->display), "%s", rhs);
	return 0;
}

int wsk_config_load(struct wsk_config *config) {
	memset(config, 0, sizeof(*config));

	const char *config_home = getenv("XDG_CONFIG_HOME");
	char path[1024];

	if (config_home && config_home[0]) {
		snprintf(path, sizeof(path), "%s/%s/%s",
				config_home, CONFIG_DIR, CONFIG_FILE);
	} else {
		const char *home = getenv("HOME");
		if (!home) return -1;
		snprintf(path, sizeof(path), "%s/.config/%s/%s",
				home, CONFIG_DIR, CONFIG_FILE);
	}

	FILE *f = fopen(path, "r");
	if (!f) return -1;

	char line[1024];
	int lineno = 0;
	int errors = 0;
	while (fgets(line, sizeof(line), f)) {
		lineno++;
		errors += parse_line(config, line, lineno);
	}
	fclose(f);

	if (config->count > 0) {
		fprintf(stderr, "Loaded %zu keymap remaps from %s\n",
				config->count, path);
	}
	return errors ? 1 : 0;
}

void wsk_config_destroy(struct wsk_config *config) {
	free(config->entries);
	memset(config, 0, sizeof(*config));
}

const struct wsk_remap *wsk_config_find(const struct wsk_config *config,
		const char *keysym) {
	for (size_t i = 0; i < config->count; i++) {
		if (strcmp(config->entries[i].keysym, keysym) == 0) {
			return &config->entries[i];
		}
	}
	return NULL;
}
