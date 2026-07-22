# mizu-pairrank

## Development

Install dependencies and start the Tauri development app:

```bash
npm ci
npm run tauri dev
```

Run the full quality check before handing off changes:

```bash
npm run check
```

## TypeScript toolchain

The project runs the current TypeScript 7 compiler through
`@typescript/native`. The `typescript` dependency points to
`@typescript/typescript6` so that tools requiring the TypeScript compiler API,
including `typescript-eslint`, can continue to use the supported TypeScript 6
API. This side-by-side setup follows the
[official TypeScript 7 migration guidance](https://devblogs.microsoft.com/typescript/announcing-typescript-7-0/#running-side-by-side-with-typescript-60).
