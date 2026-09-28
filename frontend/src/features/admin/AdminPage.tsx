import { useEffect } from 'react'
import { Link, NavLink, useLocation } from 'react-router'
import { useQueryClient } from '@tanstack/react-query'
import { useAdminAccess } from './useAdminAccess'
import { useAdminUi } from './ui'
import { AdminWork } from './work'
import { Moderation } from './Moderation'
import { Users } from './Users'
import { DesignButton } from '@/shared/ui/design-primitives'
import { AuthForm } from '@/features/auth/AuthForm'
import { useAccountFlow } from '@/features/auth/AccountFlow'
import '@/features/auth/account.css'
import './admin.css'
export default function AdminPage() {
    const admin = useAdminAccess(),
        ui = useAdminUi(),
        flow = useAccountFlow()!,
        route = useLocation(),
        client = useQueryClient()
    const { session, access } = admin
    const state =
        session.status === 'anonymous'
            ? 'anonymous'
            : session.status === 'checking'
              ? 'checking'
              : session.status === 'error'
                ? 'error'
                : access.isError
                  ? 'error'
                  : (access.data ?? 'checking')
    useEffect(() => {
        document.title = `${ui.message('Administration')} · Lycoris`
        return () => {
            document.title = 'Lycoris'
        }
    }, [ui.language])
    useEffect(() => {
        if (state !== 'ready')
            client.removeQueries({
                queryKey: admin.prefix,
                predicate: (q) => q.queryKey.at(-1) !== 'access',
            })
    }, [state, client, session.epoch])
    return (
        <main className="admin-page" lang={ui.language}>
            <header className="admin-header">
                <Link to={`/?lang=${ui.language}`} onClick={() => flow.close()}>
                    {ui.message('Back to map')}
                </Link>
                <h1 id="admin-title" tabIndex={-1}>
                    {ui.message('Administration')}
                </h1>
            </header>
            {state === 'ready' ? (
                <>
                    <nav className="admin-nav" aria-label={ui.message('Administration')}>
                        {[
                            ['/admin/review', 'Review'],
                            ['/admin/all', 'All places'],
                            ['/admin/usr', 'Users'],
                        ].map(([path, label]) => (
                            <NavLink
                                key={path}
                                to={`${path}?lang=${ui.language}`}
                                className={({ isActive }) =>
                                    isActive ||
                                    (path === '/admin/review' && route.pathname === '/admin')
                                        ? 'selected'
                                        : ''
                                }
                            >
                                {ui.message(label!)}
                            </NavLink>
                        ))}
                    </nav>
                    <AdminWork
                        key={`${session.epoch}:${session.scope?.publicId}:${route.pathname}`}
                        admin={admin}
                    >
                        {route.pathname === '/admin/usr' ? (
                            <Users />
                        ) : (
                            <Moderation all={route.pathname === '/admin/all'} />
                        )}
                    </AdminWork>
                </>
            ) : (
                <section className={`admin-gate ${state === 'anonymous' ? 'admin-login' : ''}`}>
                    {state === 'anonymous' ? (
                        <>
                            <h2>
                                {ui.message(
                                    flow.view === 'reset'
                                        ? 'Reset Password'
                                        : flow.view === 'register'
                                          ? 'Register'
                                          : 'Login',
                                )}
                            </h2>
                            <AuthForm
                                key={flow.view}
                                register={flow.view === 'register'}
                                reset={flow.view === 'reset'}
                                mobile={false}
                            />
                        </>
                    ) : (
                        <>
                            <p role="status">
                                {ui.message(
                                    state === 'denied'
                                        ? 'Access denied.'
                                        : state === 'checking'
                                          ? 'Checking access…'
                                          : 'Could not confirm access. Try again.',
                                )}
                            </p>
                            {state !== 'checking' && (
                                <DesignButton
                                    onClick={() =>
                                        session.status === 'error'
                                            ? void session.store?.refresh()
                                            : void access.refetch()
                                    }
                                >
                                    {ui.message('Try again')}
                                </DesignButton>
                            )}
                        </>
                    )}
                </section>
            )}
        </main>
    )
}
