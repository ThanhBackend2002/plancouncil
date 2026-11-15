import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { initBridge } from "./lib/bridge";
import "./index.css";

// Listener phải đăng ký ngoài React (StrictMode mount 2 lần, sẽ đăng ký trùng).
void initBridge();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
