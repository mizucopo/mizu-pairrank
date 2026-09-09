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

The project uses TypeScript 7 and type-aware Oxlint, following the repo-template
toolchain. Run `npm run lint` for lint checks or `npm run check` for the full
frontend and Rust quality gate.
