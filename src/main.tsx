import { createRoot } from "react-dom/client";
import "./styles.css";
import { Dashboard, Popover } from "./dashboard";
import { initializeLanguage, useLanguage } from "./language";
import { initializeAppearance } from "./appearance";
import { native } from "./api";

const menu = new URLSearchParams(location.search).get("surface") === "menu";
document.documentElement.dataset.surface = menu ? "menu" : "dashboard";
document.documentElement.dataset.native = String(native);
initializeAppearance(!menu);
initializeLanguage(!menu);

function App() {
  useLanguage();
  return menu ? <Popover /> : <Dashboard />;
}

createRoot(document.getElementById("root")!).render(<App />);
