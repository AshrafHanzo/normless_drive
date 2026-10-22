// Appearance, matching the desktop app's three-way choice.
//
// "system" is the default and follows the OS. The other two override it.
// Stored in localStorage so a reload does not flash the wrong theme —
// index.html starts in dark, and this corrects it before first paint.

export type Theme = "system" | "light" | "dark";

const STORAGE_KEY = "nd_theme";

export function loadTheme(): Theme {
    try {
        const stored = localStorage.getItem(STORAGE_KEY);
        if (stored === "light" || stored === "dark" || stored === "system") {
            return stored;
        }
    } catch {
        // Private windows and blocked site data both throw here. Falling
        // back to "system" is correct, not an error worth surfacing.
    }
    return "system";
}

export function applyTheme(theme: Theme) {
    const root = document.documentElement;
    const prefersDark =
        window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? true;

    const dark = theme === "dark" || (theme === "system" && prefersDark);
    root.classList.toggle("dark", dark);

    try {
        localStorage.setItem(STORAGE_KEY, theme);
    } catch {
        // Not being able to remember the choice is a lesser failure than
        // refusing to apply it.
    }
}

/// Re-applies on OS change, but only while the choice is "system".
export function watchSystemTheme(current: () => Theme) {
    const query = window.matchMedia?.("(prefers-color-scheme: dark)");
    if (!query) return () => undefined;

    const onChange = () => {
        if (current() === "system") applyTheme("system");
    };
    query.addEventListener("change", onChange);
    return () => query.removeEventListener("change", onChange);
}
