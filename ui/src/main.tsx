import { hydrateRoot } from "react-dom/client"
import { App } from "./app.tsx"
import { pageNamed } from "./logic/page.ts"

const root = document.getElementById("root")
if (root) hydrateRoot(root, <App page={pageNamed(root.dataset.page)} />)
