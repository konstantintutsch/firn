import js from "@eslint/js";
import globals from "globals";

export default [
  { ignores: ["_site/", "node_modules/"] },
  js.configs.recommended,
  // Empty catches are on purpose here: storage can fail, and nothing needs to happen.
  { rules: { "no-empty": ["error", { allowEmptyCatch: true }] } },
  {
    files: ["src/js/**/*.js"],
    languageOptions: { globals: globals.browser },
  },
  {
    // A classic script, which defines applyThemeColor for the modules.
    files: ["src/js/theme.js"],
    languageOptions: { sourceType: "script" },
  },
  {
    files: ["src/js/account.js"],
    languageOptions: { globals: { applyThemeColor: "readonly" } },
  },
  {
    files: ["src/sw.js"],
    languageOptions: { globals: globals.serviceworker },
  },
  {
    files: ["eslint.config.js", "eleventy.config.js"],
    languageOptions: { globals: globals.node },
  },
];
