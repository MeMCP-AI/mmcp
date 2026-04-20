import type { Config } from 'tailwindcss';

// Mirrored verbatim from `gui/tailwind.config.ts`. When the shared
// `packages/ui/` split ships, both apps will import this from a
// single source. Until then, changes must land in both files.
export const mmcpPreset = {
  theme: {
    extend: {
      colors: {
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
