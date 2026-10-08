import { lazy, Suspense } from 'react'
import { BrowserRouter, useLocation } from 'react-router'
import { MapPage } from './MapPage'
import { AppProviders } from './providers'
import { IcpFiling } from './IcpFiling'
const AdminPage = lazy(() => import('@/features/admin/AdminPage'))
const DevelopmentPage = import.meta.env.DEV ? lazy(() => import('./devRoutes')) : null
export function App() {
    return (
        <BrowserRouter>
            <IcpFiling>
                <Application />
            </IcpFiling>
        </BrowserRouter>
    )
}
function Application() {
    const location = useLocation()
    const lang = new URLSearchParams(location.search).get('lang')
    const isDevelopmentPath = /^\/__(dev|design)\//.test(location.pathname)
    return (
        <AppProviders languageOverride={lang === 'en' || lang === 'zh' ? lang : undefined}>
            {location.pathname === '/admin' || location.pathname.startsWith('/admin/') ? (
                <Suspense fallback={null}>
                    <AdminPage />
                </Suspense>
            ) : DevelopmentPage && isDevelopmentPath ? (
                <Suspense fallback={null}>
                    <DevelopmentPage />
                </Suspense>
            ) : (
                <MapPage />
            )}
        </AppProviders>
    )
}
