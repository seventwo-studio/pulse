// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  integrations: [
    starlight({
      title: "Pulse Docs",
      logo: { src: "./src/assets/pulse-logo.svg" },
      social: [
        {
          icon: "github",
          label: "GitHub",
          href: "https://github.com/seventwo-studio/pulse",
        },
      ],
      customCss: ["./src/styles/docs.css"],
      sidebar: [
        {
          label: "Getting Started",
          items: [
            { label: "Introduction", slug: "docs/introduction" },
            { label: "Installation", slug: "docs/installation" },
            { label: "Quick Start", slug: "docs/quickstart" },
          ],
        },
        {
          label: "Concepts",
          items: [
            { label: "Main & Workspaces", slug: "docs/concepts/main-and-workspaces" },
            { label: "Changesets & Snapshots", slug: "docs/concepts/changesets" },
            { label: "Pulse vs Git", slug: "docs/concepts/compared-to-git" },
            { label: "Sync", slug: "docs/concepts/sync" },
            { label: "Storage Engine", slug: "docs/concepts/storage" },
          ],
        },
        {
          label: "CLI Reference",
          autogenerate: { directory: "docs/cli" },
        },
        {
          label: "Architecture",
          items: [
            { label: "Overview", slug: "docs/architecture/overview" },
          ],
        },
      ],
    }),
  ],
  vite: {
    plugins: [tailwindcss()],
  },
});
