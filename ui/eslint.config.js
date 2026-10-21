// The rules that keep injected markup and code out of the wallet's pages, on
// top of the recommended ones. The CSP blocks inline script and style too;
// these catch them before a release is built.
import js from "@eslint/js"
import { defineConfig } from "eslint/config"
import reactHooks from "eslint-plugin-react-hooks"
import tseslint from "typescript-eslint"

const markup = "React's escaping stays on: render text and elements, never HTML strings"

export default defineConfig(
  { ignores: ["node_modules/", "src/generated/", ".build/"] },
  js.configs.recommended,
  tseslint.configs.recommended,
  {
    files: ["src/**/*.{ts,tsx}"],
    extends: [reactHooks.configs.flat.recommended],
    rules: { "react-hooks/exhaustive-deps": "error" },
  },
  {
    files: ["src/**/*.{ts,tsx}", "test/**/*.ts"],
    // Declared so no-implied-eval sees a string handed to them.
    languageOptions: {
      globals: { setTimeout: "readonly", setInterval: "readonly", window: "readonly" },
    },
    rules: {
      "no-eval": "error",
      "no-implied-eval": "error",
      "no-new-func": "error",
      "no-restricted-syntax": [
        "error",
        { selector: "JSXAttribute[name.name='dangerouslySetInnerHTML']", message: markup },
        { selector: "MemberExpression[property.name=/^(innerHTML|outerHTML)$/]", message: markup },
        { selector: "CallExpression[callee.property.name='insertAdjacentHTML']", message: markup },
        {
          selector:
            "CallExpression[callee.object.name='document'][callee.property.name=/^write(ln)?$/]",
          message: markup,
        },
        {
          selector: "JSXAttribute[name.name='style']",
          message: "The CSP allows no inline style: give the element a class from app.css",
        },
      ],
    },
  },
  {
    files: ["*.js", "*.mjs"],
    languageOptions: {
      globals: { console: "readonly", process: "readonly", URL: "readonly" },
    },
  },
)
