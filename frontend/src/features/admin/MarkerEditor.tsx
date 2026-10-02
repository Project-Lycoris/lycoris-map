import { useState } from 'react'
import { venueLabels } from '@/features/places/venue'
import type { Marker, VenueType } from '@/shared/api/markers'
import { markerTextSchema, type MarkerText } from '@/shared/api/markerWrites'
import { DesignButton } from '@/shared/ui/design-primitives'
import { AccountField } from '@/features/auth/accountFields'
import { categoryLabels } from '@/features/places/model'
import { useAdminUi } from './ui'
import { useAdminWork } from './work'
import { editMarker } from './api'
export function MarkerEditor({ marker, close }: { marker: Marker; close: () => void }) {
    const ui = useAdminUi(),
        work = useAdminWork(),
        [error, setError] = useState('')
    const [draft, setDraft] = useState<MarkerText>({
        title: marker.title,
        category: marker.category,
        categories: marker.categories ?? [marker.category],
        openingHoursNote: marker.openingHoursNote ?? '',
        venueType: (marker.categories ?? [marker.category]).includes('accessible_toilet')
            ? (marker.venueType ?? null)
            : null,
        description: marker.description ?? '',
        isPublic: marker.isPublic,
        openTimeStart: marker.openTimeStart ?? '',
        openTimeEnd: marker.openTimeEnd ?? '',
        language: marker.contentLanguage === 'en' ? 'en' : 'zh',
    })
    return (
        <section className="admin-card admin-editor">
            <h2>
                {ui.message('Edit approved content')} · #{marker.id}
            </h2>
            <form
                onSubmit={(event) => {
                    event.preventDefault()
                    const result = markerTextSchema.safeParse(draft)
                    if (!result.success) {
                        setError('Check the title and both opening times.')
                        return
                    }
                    setError('')
                    work.confirm({
                        label: `${ui.message('Edit approved content')} · ${draft.title}`,
                        detail: ui.message('This saves the changes as approved content.'),
                        action: async (signal) => {
                            await editMarker(marker.id, result.data, signal)
                            close()
                        },
                    })
                }}
            >
                <fieldset disabled={work.busy}>
                    <AccountField
                        label="Title"
                        required
                        maxLength={120}
                        value={draft.title}
                        onChange={(e) => setDraft({ ...draft, title: e.target.value })}
                    />
                    <fieldset className="admin-field">
                        <legend>{ui.message('Category')}</legend>
                        {Object.entries(categoryLabels).map(([value, label]) => {
                            const category = value as MarkerText['category'],
                                selected = draft.categories ?? [draft.category]
                            return (
                                <label key={value}>
                                    <input
                                        type="checkbox"
                                        checked={selected.includes(category)}
                                        onChange={() => {
                                            const next = selected.includes(category)
                                                ? selected.filter((v) => v !== category)
                                                : [...selected, category]
                                            setDraft({
                                                ...draft,
                                                category: next[0] ?? draft.category,
                                                categories: next,
                                                venueType: next.includes('accessible_toilet')
                                                    ? (draft.venueType ?? 'other')
                                                    : null,
                                            })
                                        }}
                                    />
                                    {ui.message(label)}
                                    {selected.includes(category) && selected.length > 1 && (
                                        <DesignButton
                                            disabled={selected[0] === category}
                                            onClick={() =>
                                                setDraft({
                                                    ...draft,
                                                    category,
                                                    categories: [
                                                        category,
                                                        ...selected.filter((v) => v !== category),
                                                    ],
                                                })
                                            }
                                        >
                                            {ui.message(
                                                selected[0] === category
                                                    ? 'Primary type'
                                                    : 'Make primary',
                                            )}
                                        </DesignButton>
                                    )}
                                </label>
                            )
                        })}
                    </fieldset>
                    {(draft.categories ?? [draft.category]).includes('accessible_toilet') && (
                        <label className="admin-field">
                            {ui.message('Venue type')}
                            <select
                                value={draft.venueType ?? ''}
                                onChange={(e) =>
                                    setDraft({ ...draft, venueType: e.target.value as VenueType })
                                }
                            >
                                <option value="" disabled>
                                    {ui.message('Choose a venue type')}
                                </option>
                                {Object.entries(venueLabels).map(([value, label]) => (
                                    <option value={value} key={value}>
                                        {ui.message(label)}
                                    </option>
                                ))}
                            </select>
                        </label>
                    )}
                    <label className="admin-field">
                        {ui.message('Description')}
                        <textarea
                            rows={5}
                            value={draft.description}
                            onChange={(e) => setDraft({ ...draft, description: e.target.value })}
                        />
                    </label>
                    <AccountField
                        label="Opening Time"
                        type="time"
                        value={draft.openTimeStart}
                        onChange={(e) => setDraft({ ...draft, openTimeStart: e.target.value })}
                    />
                    <AccountField
                        label="Closing Time"
                        type="time"
                        value={draft.openTimeEnd}
                        onChange={(e) => setDraft({ ...draft, openTimeEnd: e.target.value })}
                    />
                    <label className="admin-field">
                        {ui.message('Opening hours note')}
                        <textarea
                            rows={3}
                            maxLength={1000}
                            value={draft.openingHoursNote ?? ''}
                            onChange={(e) =>
                                setDraft({ ...draft, openingHoursNote: e.target.value })
                            }
                        />
                    </label>
                    <div className="admin-actions">
                        <DesignButton type="submit">{ui.message('Save')}</DesignButton>
                        <DesignButton onClick={close}>{ui.message('Cancel')}</DesignButton>
                    </div>
                </fieldset>
                {error && <p role="alert">{ui.message(error)}</p>}
            </form>
        </section>
    )
}
