import type { Config } from 'tailwindcss';

// Extracted as a named object so a future `packages/ui/` shared
// package can re-export this verbatim. Both the desktop gui and the
// eventual webui consume the same preset.
export const mmcpPreset = {
  theme: {
    extend: {
      colors: {
        // Per-kind accents used by KindBadge; also available as
        // `text-kind-rule`, `bg-kind-rule/10`, etc. across components.
        kind: {
          rule: '#60a0f0',
          snapshot: '#aa82e6',
          log: '#e6b45a',
          reference: '#5ac8be',
          scratch: '#96a0aa',
          feature: '#f0965a'
        }
      }
    }
  }
} satisfies Partial<Config>;

const config: Config = {
  content: ['./src/**/*.{html,svelte,ts}'],
  darkMode: 'class',
  presets: [mmcpPreset as Config]
};

export default config;
