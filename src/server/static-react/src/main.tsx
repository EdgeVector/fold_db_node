import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App'
import './styles/minimal-theme.css'
import { installLongTaskObserver } from './utils/longTaskObserver'

const root = document.getElementById('root')
if (!root) throw new Error('#root element not found')

if (import.meta.env.DEV) {
  installLongTaskObserver()
}

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
)
