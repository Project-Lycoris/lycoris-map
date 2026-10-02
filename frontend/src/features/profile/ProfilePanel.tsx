import { DraftList } from '@/features/contributions/DraftList'
import { AdminEntry } from '@/features/admin/AdminEntry'
import { useUi } from '@/shared/i18n/ui'
import { useRef, useState, type FormEvent } from 'react'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useSession } from '@/features/auth/SessionProvider'
import { useAccountFlow } from '@/features/auth/AccountFlow'
import { AccountAvatar } from '@/features/auth/AccountEntry'
import {
    AccountField,
    AccountSubmit,
    accountError,
    passwordError,
} from '@/features/auth/accountFields'
import { DesignButton } from '@/shared/ui/design-primitives'
import { FigmaIcon } from '@/shared/ui/figma-icon'
import { updateProfile, uploadMyAvatar } from '@/shared/api/session'
import { readCreatedPlaces } from '@/shared/api/privatePlaces'
import { privateKeys } from '@/shared/query/keys'
import type { Marker } from '@/shared/api/markers'
import type { PlaceBrowse } from '@/features/places/usePlaceBrowse'
import { ReadMessage } from '@/features/places/PlaceResults'
import { SavedPlaceRow } from '@/features/bookmarks/BookmarksPanel'
import { useMobileLayout } from '@/layouts/useMobileLayout'

export function ProfilePanel({ password = false }: { password?: boolean }) {
    const ui = useUi()
    const session = useSession(),
        flow = useAccountFlow()!
    if (!session.user || !session.scope || !session.store)
        return (
            <div className="profile-content">
                <p className="account-status" role="status">
                    {ui.message(
                        session.status === 'checking'
                            ? 'Checking your session…'
                            : (session.error ?? 'Please log in to open your account.'),
                    )}
                </p>
                <DesignButton
                    onClick={() =>
                        session.status === 'error'
                            ? void session.store?.refresh()
                            : flow.setView('login')
                    }
                >
                    {ui.text(session.status === 'error' ? 'Try again' : 'Login')}
                </DesignButton>
            </div>
        )
    return password ? (
        <PasswordForm key={`${session.scope.publicId}:${session.scope.authEpoch}`} />
    ) : (
        <ProfileForm key={`${session.scope.publicId}:${session.scope.authEpoch}`} />
    )
}
function ProfileForm() {
    const ui = useUi()
    const mobile = useMobileLayout()
    const session = useSession(),
        flow = useAccountFlow()!,
        client = useQueryClient()
    const user = session.user!,
        scope = session.scope!,
        store = session.store!
    const [nickname, setNickname] = useState(user.nickname ?? ''),
        [pronouns, setPronouns] = useState(user.pronouns ?? ''),
        [signature, setSignature] = useState(user.signature ?? '')
    const [busy, setBusy] = useState(false),
        [message, setMessage] = useState<string | null>(null)
    const running = useRef(false)
    const save = async (event: FormEvent) => {
        event.preventDefault()
        if (running.current || session.busy) return
        if (
            [...nickname.trim()].length > 255 ||
            [...pronouns.trim()].length > 64 ||
            [...signature.trim()].length > 200
        ) {
            setMessage('Please shorten the profile fields.')
            return
        }
        running.current = true
        setBusy(true)
        setMessage(null)
        try {
            const updated = await store.runPrivate(scope, (signal) =>
                updateProfile({ nickname, pronouns, signature }, signal),
            )
            store.updateUser(scope, updated)
            setMessage('Profile saved.')
        } catch (error) {
            if (store.isCurrent(scope)) setMessage(accountError(error))
        } finally {
            running.current = false
            setBusy(false)
        }
    }
    const avatar = async (file: File) => {
        if (running.current || session.busy) return
        if (
            file.size > 5 * 1024 * 1024 ||
            !['image/jpeg', 'image/png', 'image/gif', 'image/webp'].includes(file.type)
        ) {
            setMessage('Choose a JPEG, PNG, GIF or WebP image up to 5 MiB.')
            return
        }
        running.current = true
        setBusy(true)
        setMessage(null)
        try {
            const updated = await store.runPrivate(scope, (signal) => uploadMyAvatar(file, signal))
            store.updateUser(scope, updated)
            await client.invalidateQueries({ queryKey: [...privateKeys.me(scope), 'avatar'] })
            if (store.isCurrent(scope)) setMessage('Avatar updated.')
        } catch (error) {
            if (store.isCurrent(scope)) setMessage(accountError(error))
        } finally {
            running.current = false
            setBusy(false)
        }
    }
    const logout = async () => {
        if (busy || session.busy) return
        try {
            await store.logout()
            if (flow.isCurrent(flow.round)) flow.close()
        } catch (error) {
            if (flow.isCurrent(flow.round)) flow.notify(accountError(error))
        }
    }
    return (
        <div className="profile-content">
            <div className="profile-header">
                <span className="profile-avatar">
                    <AccountAvatar />
                </span>
                <div className="profile-identity">
                    <h3>{user.nickname?.trim() || ui.text('Account')}</h3>
                    <p>{user.email}</p>
                </div>
            </div>
            <label className="profile-avatar-control">
                <span>{ui.text('Change avatar')}</span>
                <input
                    className="sr-only"
                    type="file"
                    aria-label={ui.text('Change avatar')}
                    accept="image/jpeg,image/png,image/gif,image/webp"
                    disabled={busy || session.busy}
                    onChange={(event) => {
                        const file = event.target.files?.[0]
                        event.target.value = ''
                        if (file) void avatar(file)
                    }}
                />
            </label>
            <form onSubmit={(event) => void save(event)}>
                <div className="account-fields">
                    <AccountField
                        label="Nickname"
                        value={nickname}
                        onChange={(event) => setNickname(event.target.value)}
                        disabled={busy || session.busy}
                    />
                    <AccountField
                        label="Pronouns"
                        value={pronouns}
                        onChange={(event) => setPronouns(event.target.value)}
                        disabled={busy || session.busy}
                    />
                    <AccountField
                        label="Signature"
                        value={signature}
                        onChange={(event) => setSignature(event.target.value)}
                        disabled={busy || session.busy}
                    />
                </div>
                <AccountSubmit disabled={busy || session.busy}>
                    {busy ? 'Saving…' : 'Save'}
                </AccountSubmit>
            </form>
            {(message || flow.message) && (
                <p className="account-status" role="status">
                    {ui.message(message ?? flow.message)}
                </p>
            )}
            <div className="profile-options">
                <DesignButton
                    className="setting-card"
                    disabled={busy || session.busy}
                    onClick={() => {
                        flow.notify(null)
                        flow.setView('password')
                    }}
                >
                    <span>{ui.text('Change Password')}</span>
                    <span className="chevron-slot">
                        <FigmaIcon name={mobile ? 'mobileChevronBlue' : 'chevron'} />
                    </span>
                </DesignButton>
                <DesignButton
                    className="setting-card"
                    disabled={busy || session.busy}
                    onClick={() => {
                        flow.notify(null)
                        flow.setView('created')
                    }}
                >
                    <span>{ui.text('My Places')}</span>
                    <span className="chevron-slot">
                        <FigmaIcon name={mobile ? 'mobileChevronBlue' : 'chevron'} />
                    </span>
                </DesignButton>
                <AdminEntry mobile={mobile} />
            </div>
            <DesignButton
                className="profile-logout setting-card"
                disabled={busy || session.busy}
                onClick={() => void logout()}
            >
                <span>{ui.text('Logout')}</span>
                <FigmaIcon name="authLogin" />
            </DesignButton>
        </div>
    )
}
function PasswordForm() {
    const ui = useUi()
    const session = useSession(),
        flow = useAccountFlow()!
    const [oldPassword, setOld] = useState(''),
        [newPassword, setNew] = useState(''),
        [confirm, setConfirm] = useState(''),
        [message, setMessage] = useState<string | null>(null)
    const submit = async (event: FormEvent) => {
        event.preventDefault()
        if (!session.store || session.busy) return
        const invalid = passwordError(newPassword)
        if (invalid || newPassword !== confirm) {
            setMessage(invalid ?? 'The new passwords do not match.')
            return
        }
        setMessage(null)
        try {
            const user = await session.store.changePassword({ oldPassword, newPassword })
            setOld('')
            setNew('')
            setConfirm('')
            if (!flow.isCurrent(flow.round)) return
            flow.setView(user ? 'profile' : 'login')
            flow.notify(user ? 'Password changed.' : 'Password changed. Please log in again.')
        } catch (error) {
            setOld('')
            setNew('')
            setConfirm('')
            if (flow.isCurrent(flow.round)) flow.notify(accountError(error))
        }
    }
    return (
        <form className="profile-content" onSubmit={(event) => void submit(event)}>
            <div className="account-fields">
                <AccountField
                    label="Current Password"
                    type="password"
                    autoComplete="current-password"
                    required
                    value={oldPassword}
                    onChange={(event) => setOld(event.target.value)}
                    disabled={session.busy}
                />
                <AccountField
                    label="New Password"
                    type="password"
                    autoComplete="new-password"
                    required
                    value={newPassword}
                    onChange={(event) => setNew(event.target.value)}
                    disabled={session.busy}
                />
                <AccountField
                    label="Confirm Password"
                    type="password"
                    autoComplete="new-password"
                    required
                    value={confirm}
                    onChange={(event) => setConfirm(event.target.value)}
                    disabled={session.busy}
                />
            </div>
            {(message || flow.message) && (
                <p className="account-status" role="alert">
                    {ui.message(message ?? flow.message)}
                </p>
            )}
            <AccountSubmit disabled={session.busy}>
                {session.busy ? 'Saving…' : 'Save'}
            </AccountSubmit>
        </form>
    )
}
export function MyPlacesPanel({
    browse,
    mobile,
    onSelect,
}: {
    browse: PlaceBrowse
    mobile: boolean
    onSelect: (place: Marker, focus: string) => void
}) {
    const ui = useUi()
    const { store, scope, busy } = useSession()
    const query = useQuery({
        queryKey: scope ? privateKeys.created(scope, browse.language) : ['private', 'created-idle'],
        enabled: !!scope && !busy,
        queryFn: ({ signal }) =>
            store!.runPrivate(scope!, (s) => readCreatedPlaces(browse.language, s), signal),
        retry: false,
    })
    if (!scope) return <ProfilePanel />
    return (
        <div className="profile-content created-places">
            <DraftList />
            <ReadMessage
                state={{
                    pending: query.isPending,
                    error: query.isError ? 'Could not load your places. Try again.' : null,
                    retry: () => {
                        void query.refetch()
                    },
                }}
                empty={!query.data?.length}
            />
            {!query.isError && (
                <div role="list" aria-label={ui.text('My Places')}>
                    {query.data?.map((place) => (
                        <div role="listitem" key={place.id}>
                            <SavedPlaceRow
                                place={place}
                                browse={browse}
                                onSelect={onSelect}
                                mobile={mobile}
                                prefix="created"
                            />
                            <p className="created-state">
                                {ui.text(place.isPublic ? 'Public' : 'Private')} ·{' '}
                                {ui.message(place.reviewStatus.toLowerCase())}
                            </p>
                        </div>
                    ))}
                </div>
            )}
        </div>
    )
}
