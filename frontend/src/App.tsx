import { useEffect } from 'react'
import { useAuth } from './stores/auth'
import AuthPage from './components/AuthPage'
import AppShell from './components/AppShell'

export default function App() {
  const authed = useAuth((s) => s.authed)
  const hydrate = useAuth((s) => s.hydrate)

  useEffect(() => { hydrate() }, [hydrate])

  return authed ? <AppShell /> : <AuthPage />
}
