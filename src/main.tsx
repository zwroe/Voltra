import { createRoot } from "react-dom/client";
import "@fontsource-variable/manrope";
import "./styles.css";
import "./workspace.css";
import "./library.css";
import { App } from "./App";

const node = document.getElementById("root");
if (!node) throw new Error("Voltra could not find its desktop root.");
createRoot(node).render(<App />);
