// @ts-check
import { defineConfig } from 'astro/config';
import sitemap from '@astrojs/sitemap';
import tailwindcss from '@tailwindcss/vite';

// https://astro.build/config
export default defineConfig({
  site: 'https://os.itisyou.app',
  output: 'static',
  trailingSlash: 'ignore',
  integrations: [
    sitemap({
      filter: (page) => !page.includes('/404'),
    }),
  ],
  build: {
    // Keep all CSS in external files so the strict CSP (style-src 'self',
    // no 'unsafe-inline') holds.
    inlineStylesheets: 'never',
  },
  markdown: {
    // Shiki emits inline style attributes, which the site CSP forbids.
    // Code blocks are styled by the design system instead (mono on #080808).
    syntaxHighlight: false,
  },
  vite: {
    plugins: [tailwindcss()],
    build: {
      // Never inline assets as data: URIs — the CSP allows only 'self'
      // for fonts/images, so every asset must be a real file.
      assetsInlineLimit: 0,
    },
  },
});
