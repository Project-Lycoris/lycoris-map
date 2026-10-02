import { markerSchema, markerListSchema } from '@/shared/api/markers'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'
import { ContributionForm } from '@/layouts/ContributionForm'
import { syntheticPlace } from '@/features/dev/placeFixtures'
import { VenueTag } from '@/features/places/VenueTag'
import { draftFromMarker, draftText, emptyContributionDraft } from './draft'
import { markerTextSchema } from '@/shared/api/markerWrites'
afterEach(cleanup)
it('preserves an existing tag through editing and clears it when the category changes', () => {
    const draft = draftFromMarker({
        ...syntheticPlace(),
        category: 'accessible_toilet',
        venueType: 'metro',
    })
    expect(draftText(draft, 'en').venueType).toBe('metro')
    expect(
        draftText({ ...draft, category: 'medical', categories: ['medical'] }, 'en').venueType,
    ).toBeNull()
    expect(
        markerTextSchema.safeParse({ ...draftText(draft, 'en'), venueType: 'invalid' }).success,
    ).toBe(false)
    expect(
        markerTextSchema.safeParse({
            ...draftText(draft, 'en'),
            category: 'baby_room',
            categories: ['baby_room'],
        }).success,
    ).toBe(false)
})
it('shows a labelled venue selector only for an accessible toilet', () => {
    const change = vi.fn(),
        draft = { ...emptyContributionDraft, title: 'Station', category: 'toilet' as const }
    const view = render(
        <ContributionForm draft={draft} onChange={change} close={() => {}} point={null} />,
    )
    fireEvent.change(screen.getByRole('combobox', { name: 'Venue type' }), {
        target: { value: 'metro' },
    })
    expect(change).toHaveBeenCalledWith(expect.objectContaining({ venueType: 'metro' }))
    view.rerender(
        <ContributionForm
            draft={{ ...draft, category: 'nursing' }}
            onChange={change}
            close={() => {}}
            point={null}
        />,
    )
    expect(screen.queryByRole('combobox')).toBeNull()
})
it('does not mislabel nursing rooms or old responses with a missing tag', () => {
    const view = render(<VenueTag place={{ category: 'baby_room', venueType: 'metro' }} />)
    expect(view.container.textContent).toBe('')
    view.rerender(<VenueTag place={{ category: 'accessible_toilet' }} />)
    expect(view.container.textContent).toBe('')
    view.rerender(<VenueTag place={{ category: 'accessible_toilet', venueType: 'metro' }} />)
    expect(screen.getByText('Metro')).toBeInTheDocument()
})

it.each(['public_toilet', 'airport'] as const)('reads, edits and displays %s', (venueType) => {
    const place = markerSchema.parse({ ...syntheticPlace(), venueType })
    expect(draftText(draftFromMarker(place), 'en').venueType).toBe(venueType)
    const view = render(<VenueTag place={place} />)
    expect(view.container.textContent).toBe(venueType === 'airport' ? 'Airport' : 'Public toilet')
})
it('keeps future tags from breaking a list or being overwritten by unrelated edits', () => {
    const [place] = markerListSchema.parse([{ ...syntheticPlace(), venueType: 'future_venue' }])
    expect(place!.venueType).toBeNull()
    expect(draftText(draftFromMarker(place!), 'en').venueType).toBeNull()
    expect(
        markerTextSchema.safeParse({
            ...draftText(draftFromMarker(place!), 'en'),
            venueType: 'future_venue',
        }).success,
    ).toBe(false)
})
