import { invoke } from "@tauri-apps/api/core";

import { createGreetPayload } from "./lib/greeting.js";

type ElementConstructor<TElement extends Element> = new () => TElement;

function requireElement<TElement extends Element>(
  selector: string,
  elementConstructor: ElementConstructor<TElement>,
): TElement {
  const element = document.querySelector(selector);

  if (!(element instanceof elementConstructor)) {
    throw new Error(`Missing required element: ${selector}`);
  }

  return element;
}

async function submitGreeting(input: HTMLInputElement, output: HTMLOutputElement): Promise<void> {
  const message = await invoke<string>("greet", createGreetPayload(input.value));
  output.textContent = message;
}

function registerGreetingForm(): void {
  const form = requireElement("#greeting-form", HTMLFormElement);
  const input = requireElement("#greeting-name", HTMLInputElement);
  const output = requireElement("#greeting-output", HTMLOutputElement);

  form.addEventListener("submit", function handleSubmit(event: SubmitEvent): void {
    event.preventDefault();
    void submitGreeting(input, output);
  });
}

window.addEventListener("DOMContentLoaded", registerGreetingForm);
