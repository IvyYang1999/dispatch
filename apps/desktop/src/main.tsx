import React from 'react'
import ReactDOM from 'react-dom/client'
import { App } from './App'
import { Review } from './Review'
import { Waiting } from './Waiting'
import './styles.css'

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    {window.location.hash === '#review' ? <Review /> : window.location.hash === '#waiting' ? <Waiting /> : <App />}
  </React.StrictMode>,
)
