import { AllPlaces } from './AllPlaces'
import { ReviewQueue } from './ReviewQueue'
export { Pagination } from './Pagination'

/** `/admin/review` uses the master–detail queue; `/admin/all` keeps the places browser. */
export function Moderation({ all }: { all: boolean }) {
    return all ? <AllPlaces /> : <ReviewQueue />
}
