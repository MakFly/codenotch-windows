import React from "react";
import ReactDOM from "react-dom/client";
import "./notch.css";
import App from "./NotchApp";
import { isTauri } from "@tauri-apps/api/core";
// Apply transparency before the first React paint, not in a post-paint effect.
if (isTauri()) document.body.classList.add(new URLSearchParams(location.search).get("widget") === "1" ? "native-widget" : "native-settings");
ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
