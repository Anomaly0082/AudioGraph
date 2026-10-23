import { renderToStaticMarkup } from "react-dom/server";

// These behavior tests inspect button labels/disabled attributes. Decorative SVG
// markup is covered separately by visual/icon tests and contributes no accessible name.
export function renderWithoutDecorativeIcons(element) {
  return renderToStaticMarkup(element).replace(/<svg\b(?=[^>]*aria-hidden="true")[^>]*>[\s\S]*?<\/svg>/g, "");
}
