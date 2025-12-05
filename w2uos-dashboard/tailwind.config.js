/** @type {import('tailwindcss').Config} */
module.exports = {
  content: [
    "./static/**/*.{html,js}",
    "./src/**/*.{rs,html}",
  ],
  darkMode: "class",
  theme: {
    extend: {
      colors: {
        background: "#000000",
        terminal: {
          green: "#22c55e",
          amber: "#f59e0b",
          cyan: "#22d3ee",
          orange: "#f97316",
        },
      },
      fontFamily: {
        mono: ["Fira Code", "JetBrains Mono", "ui-monospace", "SFMono-Regular", "Menlo", "monospace"],
      },
      boxShadow: {
        neon: "0 0 10px rgba(34,197,94,0.45)",
      },
    },
  },
  plugins: [],
};
