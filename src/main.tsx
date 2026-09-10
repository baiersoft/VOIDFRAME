import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";

// Suppresses the webview's default browser-style right-click menu (Back/
// Reload/Inspect) so the app reads as a desktop app, not a page -- except
// over an editable element, where the OS's native Cut/Copy/Paste context
// menu is still expected. Dev builds keep the browser menu (and therefore
// "Inspect Element") for debugging; only a packaged build suppresses it.
if (!import.meta.env.DEV) {
  document.addEventListener("contextmenu", (e) => {
    const target = e.target as HTMLElement | null;
    const isEditable =
      !!target?.closest('input, textarea, [contenteditable="true"]');
    if (!isEditable) {
      e.preventDefault();
    }
  });
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
