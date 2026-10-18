import { useUi } from '@/shared/i18n/ui'
import { useEffect, useState, type FormEvent } from 'react'
import { DesignButton } from '@/shared/ui/design-primitives'
import { ApiError } from '@/shared/api/ApiError'
import { sendEmailCode } from '@/shared/api/session'
import { useAccountFlow } from './AccountFlow'
import { useSession } from './SessionProvider'
import { AccountField, AccountSubmit, accountError, passwordError } from './accountFields'
import apple from '@/assets/figma/auth-apple.svg'
import google from '@/assets/figma/auth-google.svg'

function retryDelay(error: unknown): number {
    if (!(error instanceof ApiError) || error.status !== 429) return 0
    try {
        const body: unknown = JSON.parse(error.body ?? '{}')
        if (
            body &&
            typeof body === 'object' &&
            'data' in body &&
            body.data &&
            typeof body.data === 'object' &&
            'retryAfterSeconds' in body.data
        ) {
            const seconds = body.data.retryAfterSeconds
            if (typeof seconds === 'number' && Number.isFinite(seconds) && seconds > 0)
                return Math.min(86400, seconds)
        }
    } catch {
        /* A malformed response must not hide the original error. */
    }
    return error.code === 42931 ? 3600 : 60
}

export function AuthForm({
    register,
    mobile,
    reset = false,
}: {
    register: boolean
    mobile: boolean
    reset?: boolean
}) {
    const ui = useUi(),
        session = useSession(),
        flow = useAccountFlow()!
    const [identity, setIdentity] = useState(''),
        [email, setEmail] = useState(''),
        [password, setPassword] = useState('')
    const [confirmation, setConfirmation] = useState(''),
        [code, setCode] = useState('')
    const [error, setError] = useState<string | null>(null),
        [notice, setNotice] = useState<string | null>(null)
    const [sending, setSending] = useState(false),
        [sendUntil, setSendUntil] = useState(0),
        [lockUntil, setLockUntil] = useState(0)
    const [now, setNow] = useState(Date.now)
    const verification = register || reset,
        busy = session.busy || sending
    useEffect(() => {
        if (Math.max(sendUntil, lockUntil) <= Date.now()) return
        const timer = window.setInterval(() => setNow(Date.now()), 1000)
        return () => window.clearInterval(timer)
    }, [sendUntil, lockUntil])
    const remaining = Math.max(0, Math.ceil((Math.max(sendUntil, lockUntil) - now) / 1000))
    const locked = lockUntil > now
    const failed = (error: unknown) => {
        if (!flow.isCurrent(flow.round)) return
        setError(accountError(error))
        const delay = retryDelay(error)
        if (delay) {
            const time = Date.now()
            setNow(time)
            setSendUntil(time + delay * 1000)
            // 42931 locks the verification code; 42933 locks sign-in itself.
            if (error instanceof ApiError && (error.code === 42931 || error.code === 42933))
                setLockUntil(time + delay * 1000)
        }
    }
    const send = async () => {
        if (busy || remaining) return
        const normalized = email.trim().toLowerCase()
        if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(normalized)) {
            setError('Enter a valid email address.')
            return
        }
        setSending(true)
        setError(null)
        setNotice(null)
        try {
            await sendEmailCode(normalized, reset ? 'reset_password' : 'register', ui.language)
            if (flow.isCurrent(flow.round)) {
                const time = Date.now()
                setNow(time)
                setSendUntil(time + 60000)
                setCode('')
                setNotice('A code has been sent. It expires in 10 minutes.')
            }
        } catch (error) {
            failed(error)
        } finally {
            setSending(false)
        }
    }
    const submit = async (event: FormEvent) => {
        event.preventDefault()
        if (!session.store || busy || locked) return
        if ((!reset && !identity.trim()) || !password || (verification && !email.trim())) {
            setError('Please fill in all required fields.')
            return
        }
        const invalid = verification ? passwordError(password) : null
        if (invalid) {
            setError(invalid)
            return
        }
        if (verification && !/^\d{6}$/.test(code)) {
            setError('Enter the six-digit verification code.')
            return
        }
        if (reset && confirmation !== password) {
            setError('The new passwords do not match.')
            return
        }
        setError(null)
        setNotice(null)
        try {
            if (reset) {
                await session.store.resetPassword({
                    email: email.trim().toLowerCase(),
                    verificationCode: code,
                    newPassword: password,
                })
                if (flow.isCurrent(flow.round)) {
                    flow.setView('login')
                    flow.notify('Password reset. Please log in with your new password.')
                }
            } else {
                if (register)
                    await session.store.register({
                        username: identity.trim(),
                        email: email.trim().toLowerCase(),
                        password,
                        verificationCode: code,
                    })
                else await session.store.login({ username: identity.trim(), password })
                await flow.authenticated(flow.round)
            }
            setPassword('')
            setConfirmation('')
            setCode('')
        } catch (error) {
            if (!verification) setPassword('')
            failed(error)
        }
    }
    return (
        <>
            <form className="auth-form" onSubmit={(event) => void submit(event)}>
                <div className="account-fields">
                    {verification && (
                        <AccountField
                            label="Email"
                            type="email"
                            autoComplete="email"
                            required
                            maxLength={254}
                            value={email}
                            onChange={(event) => {
                                if (
                                    event.target.value.trim().toLowerCase() !==
                                    email.trim().toLowerCase()
                                ) {
                                    setSendUntil(0)
                                    setLockUntil(0)
                                    setError(null)
                                }
                                setEmail(event.target.value)
                                setCode('')
                                setNotice(null)
                            }}
                            disabled={busy}
                            autoCapitalize="none"
                            spellCheck={false}
                        />
                    )}
                    {!reset && (
                        <AccountField
                            label={register ? 'Username' : 'Email or Username'}
                            name="username"
                            autoComplete="username"
                            required
                            maxLength={register ? 255 : undefined}
                            value={identity}
                            onChange={(event) => setIdentity(event.target.value)}
                            disabled={busy}
                            autoCapitalize="none"
                            spellCheck={false}
                        />
                    )}
                    <AccountField
                        label={reset ? 'New Password' : 'Password'}
                        name="password"
                        type="password"
                        autoComplete={verification ? 'new-password' : 'current-password'}
                        required
                        value={password}
                        onChange={(event) => setPassword(event.target.value)}
                        disabled={busy}
                    />
                    {reset && (
                        <AccountField
                            label="Confirm Password"
                            type="password"
                            autoComplete="new-password"
                            required
                            value={confirmation}
                            onChange={(event) => setConfirmation(event.target.value)}
                            disabled={busy}
                        />
                    )}
                    {verification && (
                        <div className="verification-row">
                            <AccountField
                                label="Verification Code"
                                name="verificationCode"
                                autoComplete="one-time-code"
                                inputMode="numeric"
                                pattern="[0-9]{6}"
                                maxLength={6}
                                required
                                value={code}
                                onChange={(event) =>
                                    setCode(event.target.value.replace(/\D/g, '').slice(0, 6))
                                }
                                disabled={busy || locked}
                            />
                            <DesignButton
                                className="verification-send"
                                disabled={busy || remaining > 0}
                                onClick={() => void send()}
                            >
                                {ui.text(
                                    sending
                                        ? 'Sending…'
                                        : remaining > 0
                                          ? 'Resend in {seconds}s'
                                          : 'Send code',
                                    { seconds: remaining },
                                )}
                            </DesignButton>
                        </div>
                    )}
                </div>
                <p className="auth-switch">
                    {reset ? (
                        <DesignButton
                            disabled={busy}
                            onClick={() => {
                                flow.notify(null)
                                flow.setView('login')
                            }}
                        >
                            {ui.text('Back to login')}
                        </DesignButton>
                    ) : (
                        <>
                            {ui.text(
                                register ? 'Already have an account?' : 'Haven’t got an account?',
                            )}{' '}
                            <DesignButton
                                disabled={busy}
                                onClick={() => {
                                    flow.notify(null)
                                    flow.setView(register ? 'login' : 'register')
                                }}
                            >
                                {ui.text(register ? 'Login Here.' : 'Register Here.')}
                            </DesignButton>
                        </>
                    )}
                    {!verification && (
                        <DesignButton
                            className="auth-forgot"
                            disabled={busy}
                            onClick={() => {
                                flow.notify(null)
                                flow.setView('reset')
                            }}
                        >
                            {ui.text('Forgot password?')}
                        </DesignButton>
                    )}
                </p>
                {(error || flow.message) && (
                    <p className="account-status" role="alert">
                        {ui.message(error ?? flow.message)}
                    </p>
                )}
                {notice && (
                    <p className="account-status" role="status">
                        {ui.message(notice)}
                    </p>
                )}
                <AccountSubmit disabled={busy || locked} aria-busy={busy}>
                    {locked
                        ? ui.text('Try again in {seconds}s', { seconds: remaining })
                        : busy
                          ? 'Please wait…'
                          : reset
                            ? 'Reset Password'
                            : register
                              ? 'Register'
                              : 'Login'}
                </AccountSubmit>
            </form>
            {!reset && (
                <div className="auth-social">
                    <h3>{ui.text(mobile ? 'Third Party Login' : '--OR--')}</h3>
                    <div className="auth-social-buttons">
                        <DesignButton
                            className="auth-apple"
                            onClick={() =>
                                setError(
                                    'Apple login is not available yet. Please use your email or username.',
                                )
                            }
                        >
                            <img src={apple} alt="" width={24} height={24} />
                            <span>{ui.text('Continue with Apple')}</span>
                        </DesignButton>
                        <DesignButton
                            className="auth-google"
                            onClick={() =>
                                setError(
                                    'Google login is not available yet. Please use your email or username.',
                                )
                            }
                        >
                            <img src={google} alt="" width={23} height={23} />
                            <span>{ui.text('Continue with Google')}</span>
                        </DesignButton>
                    </div>
                </div>
            )}
        </>
    )
}
