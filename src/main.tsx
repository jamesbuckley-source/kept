import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { QuickPicker } from "./components/QuickPicker";
import { windowKind } from "./lib/windowKind";
import "./index.css";
import { installWindowDrag } from "./lib/windowDrag";

installWindowDrag();

// The `::` popup is a single small surface with none of the history chrome, so it
// is mounted on its own rather than paying for the main application's state: it
// opens on a keystroke, and its list has to be there immediately.
const quick = windowKind() === "quick";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>{quick ? <QuickPicker /> : <App />}</React.StrictMode>,
);
