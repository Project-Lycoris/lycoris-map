import { venueLabels } from '@/features/places/venue'
import type { VenueType } from '@/shared/api/markers'
import { useUi } from '@/shared/i18n/ui'
import { useEffect, useId, useRef } from 'react'
import { CategoryBadge, DesignButton, IconButton } from './primitives'
import { FigmaIcon } from '@/shared/ui/figma-icon'
import './contribution-form.css'
import {
    draftCategories,
    draftPhotos,
    type ContributionDraft,
} from '@/features/contributions/draft'
import {
    contributionBusy,
    type ContributionSnapshot,
} from '@/features/contributions/ContributionStore'
export { emptyContributionDraft, type ContributionDraft } from '@/features/contributions/draft'

export type ContributionFormProps = {
    draft: ContributionDraft
    onChange: (draft: ContributionDraft) => void
    close: () => void
    point: { lat: number; lng: number } | null
    mobile?: boolean
    state?: ContributionSnapshot | null | undefined
    onSubmit?: ((resendUnconfirmed?: boolean) => void) | undefined
    onPhoto?: ((file: File | null) => void) | undefined
    onPhotos?: ((files: File[]) => void) | undefined
    onRemovePhoto?: ((index: number) => void) | undefined
    onView?: (() => void) | undefined
}

export function ContributionForm({
    draft,
    onChange,
    close,
    point,
    mobile = false,
    state,
    onSubmit,
    onPhoto,
    onPhotos,
    onRemovePhoto,
    onView,
}: ContributionFormProps) {
    const ui = useUi()
    const selected = draftCategories(draft),
        photos = draftPhotos(draft)
    const id = useId()
    const heading = useRef<HTMLHeadingElement>(null)
    const photoInput = useRef<HTMLInputElement>(null)
    const feedback = useRef<HTMLDivElement>(null)
    const submit = useRef<HTMLButtonElement>(null)
    useEffect(() => {
        heading.current?.focus({ preventScroll: true })
    }, [])
    useEffect(() => {
        if (state?.phase === 'complete') {
            submit.current?.focus({ preventScroll: true })
            submit.current?.scrollIntoView?.({ block: 'nearest' })
        } else if (state?.error) feedback.current?.scrollIntoView?.({ block: 'nearest' })
    }, [state?.error, state?.phase])
    const locked = !!state && state.phase !== 'draft'
    const busy = !!state && contributionBusy(state.phase)
    const unconfirmed = state?.phase === 'save-uncertain' && !!state.base
    const action =
        state?.phase === 'complete'
            ? 'View place'
            : busy
              ? 'Saving…'
              : state?.saved
                ? 'Retry photo'
                : state?.phase === 'save-uncertain'
                  ? 'Retry'
                  : 'Submit'
    const update = <K extends keyof ContributionDraft>(key: K, value: ContributionDraft[K]) =>
        onChange({ ...draft, [key]: value })
    const categories = [
        ['toilet', 'Accessible Toilets'],
        ['nursing', 'Nursing Rooms'],
        ['medical', 'Medical Institutions'],
    ] as const
    return (
        <form
            className={`contribution-form contribution-form-flow ${mobile ? 'contribution-form-mobile' : ''} ${state ? 'contribution-form-live' : ''} ${selected.includes('toilet') ? 'has-venue' : ''}`}
            aria-label={ui.text(state?.base ? 'Edit proposal' : 'Contribution draft')}
            aria-busy={busy || undefined}
            data-lat={import.meta.env.DEV ? point?.lat : undefined}
            data-lng={import.meta.env.DEV ? point?.lng : undefined}
            onSubmit={(event) => {
                event.preventDefault()
                if (busy || unconfirmed) return
                if (state?.phase === 'complete') onView?.()
                else onSubmit?.()
            }}
        >
            <h1 ref={heading} tabIndex={-1}>
                {ui.text(state?.base ? 'Edit place' : 'Contribute')}
            </h1>
            <IconButton
                className="contribution-close"
                icon="close"
                label="Close contribution form"
                onClick={close}
            />
            <label className="contribution-label title-label" htmlFor={`${id}-title`}>
                {ui.text('Title')}
            </label>
            <input
                id={`${id}-title`}
                className="contribution-input contribution-title"
                placeholder={mobile ? ui.text('Input title here') : undefined}
                value={draft.title}
                disabled={locked}
                onChange={(event) => update('title', event.target.value)}
                autoComplete="off"
            />
            <span id={`${id}-category`} className="contribution-label category-label">
                {ui.text('Category')}
            </span>
            <div
                className="contribution-categories"
                role="group"
                aria-labelledby={`${id}-category`}
            >
                {categories.map(([category, label]) => (
                    <div key={category} className="contribution-type">
                        <DesignButton
                            role="checkbox"
                            aria-label={ui.message(label)}
                            aria-checked={selected.includes(category)}
                            disabled={locked}
                            onClick={() => {
                                const next = selected.includes(category)
                                    ? selected.filter((value) => value !== category)
                                    : [...selected, category]
                                onChange({
                                    ...draft,
                                    category: next[0] ?? null,
                                    categories: next,
                                    venueType: next.includes('toilet')
                                        ? (draft.venueType ?? 'other')
                                        : null,
                                })
                            }}
                        >
                            <CategoryBadge category={category} />
                            {selected.includes(category) && (
                                <span className="type-order">{selected.indexOf(category) + 1}</span>
                            )}
                        </DesignButton>
                        {selected.length > 1 && selected.includes(category) && (
                            <DesignButton
                                className="primary-type"
                                disabled={locked || selected[0] === category}
                                onClick={() =>
                                    onChange({
                                        ...draft,
                                        category,
                                        categories: [
                                            category,
                                            ...selected.filter((value) => value !== category),
                                        ],
                                    })
                                }
                            >
                                {ui.text(
                                    selected[0] === category ? 'Primary type' : 'Make primary',
                                )}
                            </DesignButton>
                        )}
                    </div>
                ))}
                {draft.category === 'custom' && (
                    <span className="contribution-custom-category">{ui.text('Custom')}</span>
                )}
            </div>
            <p className="contribution-help">
                {ui.text('Select all facilities here. The first type sets the pin color.')}
            </p>
            {selected.includes('toilet') && (
                <div className="contribution-venue">
                    <label className="contribution-label" htmlFor={`${id}-venue`}>
                        {ui.text('Venue type')}
                    </label>
                    <select
                        id={`${id}-venue`}
                        className="contribution-input"
                        value={draft.venueType ?? ''}
                        disabled={locked}
                        onChange={(event) => update('venueType', event.target.value as VenueType)}
                    >
                        <option value="" disabled>
                            {ui.text('Choose a venue type')}
                        </option>
                        {Object.entries(venueLabels).map(([value, label]) => (
                            <option value={value} key={value}>
                                {ui.message(label)}
                            </option>
                        ))}
                    </select>
                </div>
            )}
            <label className="contribution-label description-label" htmlFor={`${id}-description`}>
                {ui.text('Description')}
            </label>
            <textarea
                id={`${id}-description`}
                className="contribution-input contribution-description"
                value={draft.description}
                disabled={locked}
                onChange={(event) => update('description', event.target.value)}
            />
            {(['opening', 'closing'] as const).map((kind) => {
                const label = kind === 'opening' ? 'Opening Time' : 'Closing Time'
                return (
                    <div
                        className={`contribution-time ${kind}-time`}
                        key={kind}
                        role="group"
                        aria-labelledby={`${id}-${kind}`}
                    >
                        <span id={`${id}-${kind}`} className="contribution-label">
                            {ui.message(label)}
                        </span>
                        <div className="time-inputs">
                            {(['Hour', 'Minute'] as const).map((part, index) => (
                                <span className="time-part" key={part}>
                                    {index === 1 && (
                                        <span aria-hidden="true" className="time-colon">
                                            :
                                        </span>
                                    )}
                                    <input
                                        className="contribution-input"
                                        aria-label={ui.message(`${label} ${part.toLowerCase()}`)}
                                        inputMode="numeric"
                                        autoComplete="off"
                                        maxLength={2}
                                        pattern={
                                            part === 'Hour' ? '([01]?[0-9]|2[0-3])' : '[0-5]?[0-9]'
                                        }
                                        value={draft[`${kind}${part}`]}
                                        disabled={locked}
                                        onChange={(event) =>
                                            update(
                                                `${kind}${part}`,
                                                event.target.value.replace(/\D/g, '').slice(0, 2),
                                            )
                                        }
                                        onBlur={(event) => {
                                            const value = event.currentTarget.value
                                            if (value && event.currentTarget.validity.valid)
                                                update(`${kind}${part}`, value.padStart(2, '0'))
                                        }}
                                    />
                                </span>
                            ))}
                        </div>
                    </div>
                )
            })}
            <label className="contribution-label" htmlFor={`${id}-hours-note`}>
                {ui.text('Opening hours note')}
            </label>
            <textarea
                id={`${id}-hours-note`}
                className="contribution-input contribution-hours-note"
                rows={3}
                maxLength={1000}
                value={draft.openingHoursNote ?? ''}
                disabled={locked}
                onChange={(event) => update('openingHoursNote', event.target.value)}
            />
            <span className="contribution-label upload-label" id={`${id}-photo-label`}>
                {ui.text('Upload Photo (Optional)')}
            </span>
            <input
                ref={photoInput}
                type="file"
                multiple
                accept="image/jpeg,image/png,image/gif,image/webp"
                aria-labelledby={`${id}-photo-label`}
                hidden
                onChange={(event) => {
                    const files = Array.from(event.target.files ?? [])
                    if (files.length) {
                        if (onPhotos) onPhotos(files)
                        else if (onPhoto) onPhoto(files[0]!)
                        else onChange({ ...draft, photo: files[0]!, photos: [...photos, ...files] })
                    }
                    event.target.value = ''
                }}
            />
            <DesignButton
                className="contribution-action contribution-upload"
                aria-label={
                    draft.photo
                        ? ui.text('Upload photo: {name} selected', { name: draft.photo.name })
                        : ui.text('Upload photo')
                }
                disabled={!!state && !['draft', 'photo-error'].includes(state.phase)}
                onClick={() => photoInput.current?.click()}
            >
                <span>{ui.text('Upload')}</span>
                <FigmaIcon name="upload" />
            </DesignButton>
            {state && (
                <div className="contribution-feedback" ref={feedback}>
                    {state.base && (
                        <p>{ui.text('Location is fixed. Changes are submitted for review.')}</p>
                    )}
                    {photos.map((photo, index) => (
                        <p className="contribution-photo-name" key={`${index}:${photo.name}`}>
                            {index + 1}. {photo.name}
                            {index < state.uploadedCount
                                ? ' ✓'
                                : ['draft', 'photo-error'].includes(state.phase) && (
                                      <DesignButton
                                          onClick={() =>
                                              onRemovePhoto ? onRemovePhoto(index) : onPhoto?.(null)
                                          }
                                      >
                                          {ui.text('Remove')}
                                      </DesignButton>
                                  )}
                        </p>
                    ))}
                    {state.persistenceError && (
                        <p role="alert">{ui.message(state.persistenceError)}</p>
                    )}
                    <p role={state.error ? 'alert' : 'status'}>
                        {ui.message(
                            state.error ??
                                (state.phase === 'complete'
                                    ? state.base
                                        ? 'Your changes have been submitted for review.'
                                        : 'Place saved. Public places appear on the map after review.'
                                    : state.phase === 'photo-saving'
                                      ? 'Place saved. Uploading photo…'
                                      : state.phase === 'checking-photo'
                                        ? 'Checking photo…'
                                        : ''),
                        )}
                    </p>
                    {unconfirmed && (
                        <DesignButton
                            className="contribution-resend"
                            onClick={() => onSubmit?.(true)}
                        >
                            {ui.text('Send again')}
                        </DesignButton>
                    )}
                </div>
            )}
            <DesignButton
                className="contribution-action contribution-submit"
                ref={submit}
                type="submit"
                disabled={!!state && (busy || !!unconfirmed)}
                available={!!onSubmit && !busy && !unconfirmed}
            >
                <span>{ui.message(action)}</span>
                <FigmaIcon name="send" />
            </DesignButton>
        </form>
    )
}
