# TypeScript guidance

## Tauri

- Frontend sources live in `src/`; the Rust application shell lives in `src-tauri/`.
- Treat `dist/` and `src-tauri/target/` as generated output.
- Run the complete repository quality gate from the repository root for frontend or Rust shell changes:

```bash
npm run check
```
