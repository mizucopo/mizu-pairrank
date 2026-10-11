declare module "*.otf?inline" {
  const dataUrl: string;
  export default dataUrl;
}

declare module "*.ttf?inline" {
  const dataUrl: string;
  export default dataUrl;
}

declare module "*.txt?raw" {
  const text: string;
  export default text;
}
