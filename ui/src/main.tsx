import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
// React Flow's own styles, bundled with the app (no runtime style injection).
import '@xyflow/react/dist/style.css'
import './index.css'
import './app.css'
import App from './App.tsx'

const root = document.getElementById('root')
if (root) {
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  )
}
