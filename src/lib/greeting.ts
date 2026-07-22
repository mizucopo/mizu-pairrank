export type GreetPayload = {
  name: string;
};

export function createGreetPayload(rawName: string): GreetPayload {
  const name = rawName.trim();

  if (name.length === 0) {
    return { name: "Tauri" };
  }

  return { name };
}
