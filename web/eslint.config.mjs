// @ts-check

import angular from "angular-eslint";
import eslint from "@eslint/js";
import eslintConfigPrettier from "eslint-config-prettier/flat";
import { defineConfig } from "eslint/config";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";
import tseslint from "typescript-eslint";

const tsconfigRootDir = dirname(fileURLToPath(import.meta.url));

export default defineConfig(
  {
    ignores: [".angular/**", "coverage/**", "dist/**", "projects/parallax/src/lib/proto/**", "test-results/**", "playwright-report/**"] // Skip generated Angular, coverage, build output and protobuf code.
  },
  {
    linterOptions: {
      reportUnusedDisableDirectives: "error", // Reject stale eslint-disable comments.
      reportUnusedInlineConfigs: "error" // Reject inline rule settings that no longer change behavior.
    }
  },
  {
    files: ["**/*.ts"],
    extends: [
      eslint.configs.recommended, // Catch common JavaScript correctness errors.
      ...tseslint.configs.strictTypeChecked, // Enable the strict type-aware correctness rules.
      ...tseslint.configs.stylisticTypeChecked, // Prefer clear, modern TypeScript constructs.
      ...angular.configs.tsRecommended // Apply Angular's recommended component and lifecycle checks.
    ],
    processor: angular.processInlineTemplates, // Lint inline templates with the external-template rules.
    languageOptions: {
      parserOptions: {
        projectService: true, // Reuse TypeScript's project service for accurate type information.
        tsconfigRootDir // Resolve project configurations from this repository.
      }
    },
    rules: {
      complexity: ["error", 15], // Limit cyclomatic complexity.
      curly: ["error", "all"], // Require braces around every control-flow body.
      eqeqeq: ["error", "always"], // Require strict equality and inequality.
      "guard-for-in": "error", // Require inherited keys to be filtered in for-in loops.
      "no-param-reassign": ["error", { props: true }], // Prevent mutation through function parameters.
      "no-promise-executor-return": "error", // Reject misleading values returned from Promise executors.
      // Reject the error-prone comma operator.
      "no-restricted-syntax": ["error", { selector: "SequenceExpression", message: "Do not use the comma operator; split the expressions into explicit statements." }],
      "no-return-assign": ["error", "always"], // Keep assignment side effects out of return expressions.
      "prefer-object-has-own": "error", // Use the safe own-property check.
      "require-atomic-updates": "error", // Catch stale read-modify-write updates across awaits.
      "@typescript-eslint/consistent-type-exports": "error", // Mark type-only exports explicitly.
      "@typescript-eslint/consistent-type-imports": ["error", { prefer: "type-imports", fixStyle: "separate-type-imports" }], // Keep type-only dependencies out of runtime imports.
      // Require return contracts while preserving contextual callback inference.
      "@typescript-eslint/explicit-function-return-type": ["error", { allowConciseArrowFunctionExpressionsStartingWithVoid: false, allowExpressions: true, allowTypedFunctionExpressions: true }],
      "@typescript-eslint/explicit-member-accessibility": ["error", { accessibility: "explicit", overrides: { constructors: "no-public" } }], // Make class API visibility explicit.
      "@typescript-eslint/explicit-module-boundary-types": "error", // Type exported function and public method boundaries.
      "@typescript-eslint/no-floating-promises": ["error", { checkThenables: true, ignoreIIFE: false, ignoreVoid: false }], // Require every promise and thenable to be awaited or rejection-handled.
      "@typescript-eslint/no-import-type-side-effects": "error", // Prevent type-only imports from emitting side effects.
      "@typescript-eslint/no-loop-func": "error", // Prevent closures from capturing unsafe loop state.
      "@typescript-eslint/no-unsafe-type-assertion": "error", // Reject assertions that narrow without proof.
      "@typescript-eslint/prefer-readonly": "error", // Mark never-reassigned private members readonly.
      "@typescript-eslint/require-array-sort-compare": ["error", { ignoreStringArrays: false }], // Require explicit ordering for every array sort.
      "@typescript-eslint/strict-boolean-expressions": [
        "error",
        {
          allowAny: false,
          allowNullableBoolean: false,
          allowNullableEnum: false,
          allowNullableNumber: false,
          allowNullableObject: false,
          allowNullableString: false,
          allowNumber: false,
          allowString: false
        }
      ], // Require conditions to be actual booleans.
      "@typescript-eslint/strict-void-return": "error", // Reject discarded return values through void callbacks.
      // Require exhaustive union switches and defaults for open-ended values.
      "@typescript-eslint/switch-exhaustiveness-check": ["error", { allowDefaultCaseForExhaustiveSwitch: false, considerDefaultExhaustiveForUnions: false, requireDefaultForNonUnion: true }],
      "@angular-eslint/computed-must-return": "error", // Require every computed signal to produce a value.
      "@angular-eslint/no-async-lifecycle-method": "error", // Reject lifecycle methods Angular will not await.
      "@angular-eslint/no-duplicates-in-metadata-arrays": "error", // Reject duplicate Angular metadata entries.
      "@angular-eslint/no-implicit-take-until-destroyed": "error", // Require an explicit DestroyRef outside injection contexts.
      "@angular-eslint/no-lifecycle-call": "error", // Prevent application code from invoking lifecycle hooks.
      "@angular-eslint/no-pipe-impure": "error", // Reject repeatedly evaluated impure pipes.
      "@angular-eslint/no-uncalled-signals": "error", // Catch code that uses a signal object instead of its value.
      "@angular-eslint/prefer-output-readonly": "error", // Keep output references from being reassigned.
      "@angular-eslint/use-injectable-provided-in": "error", // Require tree-shakable injectable registration.
      "@angular-eslint/use-lifecycle-interface": "error" // Require lifecycle method contracts.
    }
  },
  {
    files: ["**/*.html"],
    extends: [
      ...angular.configs.templateRecommended, // Apply Angular's recommended template correctness checks.
      ...angular.configs.templateAccessibility // Apply Angular's full accessibility preset.
    ],
    rules: {
      "@angular-eslint/template/button-has-type": "error", // Prevent accidental form submission buttons.
      "@angular-eslint/template/conditional-complexity": "error", // Limit nested template conditionals.
      "@angular-eslint/template/cyclomatic-complexity": "error", // Limit independent template execution paths.
      "@angular-eslint/template/no-any": "error", // Forbid the $any template type escape hatch.
      "@angular-eslint/template/no-duplicate-attributes": "error", // Reject duplicate inputs and event handlers.
      "@angular-eslint/template/no-empty-control-flow": "error", // Reject unfinished empty control-flow blocks.
      "@angular-eslint/template/no-interpolation-in-attributes": "error", // Use typed property binding for attributes.
      "@angular-eslint/template/no-non-null-assertion": "error", // Forbid template non-null assertions.
      "@angular-eslint/template/no-outerhtml": "error", // Prevent DOM replacement outside Angular's view model.
      "@angular-eslint/template/prefer-ngsrc": "error", // Use Angular's optimized image directive.
      "@angular-eslint/template/require-switch-default": "error" // Handle unmatched template switch values.
    }
  },
  eslintConfigPrettier // Disable lint rules that conflict with Prettier.
);
