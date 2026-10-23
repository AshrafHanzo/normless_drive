// Theme presets, matching the set the desktop app ships.
//
// A preset overrides the design tokens defined in index.css by setting the
// same custom properties inline on :root. That keeps one source of truth
// for what a token *means* — every component still reads `--color-ink`,
// and a preset only changes what colour that resolves to.
//
// `dark` tells the app which base to sit on, so icons, shadows and the
// browser's own form controls match rather than fighting the palette.

export interface Preset {
  id: string;
  name: string;
  dark: boolean;
  /// The three swatches shown on the picker card: ground, raised, accent.
  swatch: [string, string, string];
  tokens: Record<string, string>;
}

/// Builds a full token set from a small palette, so a preset only has to
/// state what makes it distinctive rather than redeclare twenty variables
/// — and so adding a token later does not mean editing every preset.
function build(p: {
  canvas: string;
  surface: string;
  raised: string;
  border: string;
  borderSoft: string;
  ink: string;
  inkMuted: string;
  inkFaint: string;
  accent: string;
  accentHover: string;
  accentSoft: string;
}): Record<string, string> {
  return {
    "--color-canvas": p.canvas,
    "--color-surface": p.surface,
    "--color-raised": p.raised,
    "--color-border": p.border,
    "--color-border-soft": p.borderSoft,
    "--color-ink": p.ink,
    "--color-ink-muted": p.inkMuted,
    "--color-ink-faint": p.inkFaint,
    "--color-accent": p.accent,
    "--color-accent-hover": p.accentHover,
    "--color-accent-soft": p.accentSoft,
  };
}

export const PRESETS: Preset[] = [
  {
    id: "default-dark",
    name: "Default Dark",
    dark: true,
    swatch: ["#0d0d0f", "#1a1a1f", "#4a92ff"],
    tokens: build({
      canvas: "#0d0d0f",
      surface: "#141417",
      raised: "#1a1a1f",
      border: "#2a2a31",
      borderSoft: "#202026",
      ink: "#f2f2f4",
      inkMuted: "#9e9ea8",
      inkFaint: "#6b6b76",
      accent: "#4a92ff",
      accentHover: "#68a5ff",
      accentSoft: "#16233a",
    }),
  },
  {
    id: "charcoal",
    name: "Charcoal",
    dark: true,
    swatch: ["#17171f", "#21212e", "#7c5cff"],
    tokens: build({
      canvas: "#17171f",
      surface: "#1c1c26",
      raised: "#21212e",
      border: "#32324a",
      borderSoft: "#27273a",
      ink: "#ececf5",
      inkMuted: "#9f9fb8",
      inkFaint: "#6c6c85",
      accent: "#7c5cff",
      accentHover: "#9478ff",
      accentSoft: "#241f45",
    }),
  },
  {
    id: "nord",
    name: "Nord",
    dark: true,
    swatch: ["#2e3440", "#3b4252", "#88c0d0"],
    tokens: build({
      canvas: "#2e3440",
      surface: "#343c4a",
      raised: "#3b4252",
      border: "#4c566a",
      borderSoft: "#434c5e",
      ink: "#eceff4",
      inkMuted: "#b8c0cc",
      inkFaint: "#8793a5",
      accent: "#88c0d0",
      accentHover: "#9fd0de",
      accentSoft: "#394854",
    }),
  },
  {
    id: "monokai",
    name: "Monokai",
    dark: true,
    swatch: ["#1e1f1c", "#2a2b27", "#a6e22e"],
    tokens: build({
      canvas: "#1e1f1c",
      surface: "#252620",
      raised: "#2a2b27",
      border: "#3e3f38",
      borderSoft: "#32332d",
      ink: "#f8f8f2",
      inkMuted: "#b5b5a8",
      inkFaint: "#80806f",
      accent: "#a6e22e",
      accentHover: "#b9eb57",
      accentSoft: "#2f3a1c",
    }),
  },
  {
    id: "cyber-teal",
    name: "Cyber Teal",
    dark: true,
    swatch: ["#071221", "#0e2036", "#00e5b0"],
    tokens: build({
      canvas: "#071221",
      surface: "#0b1a2c",
      raised: "#0e2036",
      border: "#1c3552",
      borderSoft: "#152a44",
      ink: "#e6f6ff",
      inkMuted: "#8fb3cc",
      inkFaint: "#5d7f99",
      accent: "#00e5b0",
      accentHover: "#3dedc4",
      accentSoft: "#0a3a33",
    }),
  },
  {
    id: "default-light",
    name: "Default Light",
    dark: false,
    swatch: ["#ffffff", "#f7f7f8", "#2d7ff9"],
    tokens: build({
      canvas: "#ffffff",
      surface: "#f7f7f8",
      raised: "#ffffff",
      border: "#e4e4e7",
      borderSoft: "#f0f0f1",
      ink: "#18181b",
      inkMuted: "#5c5c66",
      inkFaint: "#8e8e98",
      accent: "#2d7ff9",
      accentHover: "#1f6fe5",
      accentSoft: "#e8f1fe",
    }),
  },
  {
    id: "solarized-light",
    name: "Solarized Light",
    dark: false,
    swatch: ["#fdf6e3", "#eee8d5", "#b58900"],
    tokens: build({
      canvas: "#fdf6e3",
      surface: "#f5eedb",
      raised: "#fffbf0",
      border: "#e0d8c0",
      borderSoft: "#efe8d4",
      ink: "#073642",
      inkMuted: "#586e75",
      inkFaint: "#93a1a1",
      accent: "#b58900",
      accentHover: "#9c7600",
      accentSoft: "#f3e9cc",
    }),
  },
];

const STORAGE_KEY = "nd_preset";

export function loadPreset(): string | null {
  try {
    return localStorage.getItem(STORAGE_KEY);
  } catch {
    return null;
  }
}

/// Applies a preset, or clears it so the stylesheet's own tokens apply.
///
/// Clearing removes the inline properties rather than writing defaults
/// back: index.css already has light and dark values with media queries,
/// and overwriting them inline would freeze whichever one happened to be
/// active at the time.
export function applyPreset(id: string | null) {
  const root = document.documentElement;
  const preset = PRESETS.find((p) => p.id === id);

  for (const key of Object.keys(PRESETS[0].tokens)) {
    root.style.removeProperty(key);
  }

  try {
    if (preset) {
      localStorage.setItem(STORAGE_KEY, preset.id);
    } else {
      localStorage.removeItem(STORAGE_KEY);
    }
  } catch {
    // Not remembering the choice is better than not applying it.
  }

  if (!preset) return;

  for (const [key, value] of Object.entries(preset.tokens)) {
    root.style.setProperty(key, value);
  }
  // Keeps shadows, scrollbars and native controls on the right side.
  root.classList.toggle("dark", preset.dark);
}
