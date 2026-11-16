# Conversation-first UI + tổng hợp lý do reject (bản tối ưu)

> Rút gọn từ `conversation-first-ui.v3.md` sau khi đối chiếu code thật. Chỉ đổi frontend `src/`; không đổi IPC, `models.rs`, orchestrator.

## 1. Overview

**Mục tiêu**
1. Màn họp ưu tiên hội thoại: `MeetingRoom` + `Timeline` là vùng chính, bỏ cột plan luôn hiển thị.
2. Plan chỉ mở theo yêu cầu, trong drawer bọc `PlanViewer` hiện có (xem / copy / export / diff / token đã có sẵn).
3. Một nơi tóm tắt vì sao từng version bị reject, theo từng round.

**Hiện trạng đã xác minh** (thay cho "Bước 0")
- `CouncilView.tsx:42-63`: grid `[minmax(440px,2fr)_3fr]`; cột trái = StateBanner, PromptPanel, ScopeBar, MeetingRoom, MemberControls, ProgressBar, Timeline; cột phải = `<PlanViewer scope resolve/>`. `App.tsx:20-24` giữ cả 3 tab luôn mounted (tab ẩn bằng `hidden`).
- `PlanViewer.tsx:30-40`: state chọn version là `sel` (nội bộ); `planFocus` từ `useUi` được áp bằng `useEffect` (cả lúc mount). `copy`/`exportMd` (`:93-112`) đã đọc text qua `contentOf` -> `planText`, đã tắt khi đang stream (`canAct`). Đã có tab Final/Diff/Token. Không cần thêm prop `selectedVersion`.
- `PlanViewer` -> `Markdown.tsx:81-92`: heading **không có id/anchor**, chỉ có badge section + nút lọc/so sánh (`deriveSectionStatus`). Không có cuộn tới heading; badge `changes_requested` theo section **đã có sẵn** trong plan.
- `ui.ts:11-12,79`: `focusPlan(version)` **chỉ nhận version, không nhận scopeId**. Call site duy nhất: `Timeline.tsx:296,309` (nút "Xem vN"). Người dùng đang xem đúng scope của Timeline nên không cần map scope.
- `Dialog.tsx`: portal, `role="dialog"`, `aria-modal`, focus trap, trả focus về phần tử cũ, Esc đóng với `stopPropagation` ở capture (`:50-79`). Chỉ hỗ trợ căn giữa `max-w-lg`; muốn làm drawer phải thêm prop.
- Esc dừng họp: `useHotkeys.ts:72-75,108-114` (listener `window` bubble). `canStopHotkey` đã bỏ qua phím phát từ `[role="dialog"]` (`POPUP`), và Dialog chặn Esc ở capture -> **không cần guard** `planDrawer.open`; xác nhận dừng họp (`PromptPanel.tsx:46-75`) chỉ mở bằng Esc nên không thể chồng lên drawer.
- `Timeline.tsx:84-86`: `sectionFilter` do `PlanViewer` đặt (`onFilter`); `Timeline` tự reset khi đổi scope/phiên. Timeline có sẵn nút "Phóng to" mở `Dialog` (`:105-140`) và dòng tiêu đề round có badge approve/reject (`:468`). Mỗi `ReviewMsg` đã mở rộng được critical_issues/suggestions.
- Dữ liệu review (`session.ts:59-80`): `PlanView {version, markdown, complete}`; `SlotView {review: Review|null, error: string|null, pending}`; `RoundView {round, planVersion, slots: Record<RoleId,SlotView>, approvals, total, converged, completed}`. `Review` (`contracts.ts:40-54`): `verdict: "APPROVE"|"REJECT"|"ABSTAIN"`, `summary_bubble`, `critical_issues`, `suggestions`, `sections?`, `carried_over?` (chỉ là APPROVE giữ nguyên). Không có verdict "lạ" (type đóng).
- **History đủ dữ liệu**: `council.ts:120` `openRecord` -> `fromRecord` -> `scopeFromRecord` (`session.ts:~716-750`) dựng `RoundView.slots[...].review` đầy đủ + `converged`. Phiên xem lại có `readOnly=true`, `viewing != null`.
- `ui.ts:100-118,137-138`: pref lưu localStorage bằng `readPref`/`writePref` (key `plancouncil.*`); trang cài đặt sẵn có `settings/InterfaceTab.tsx` (theme, motion, dùng `RadioCards`).
- `lib/sectionStatus.ts:54-58,66-85`: `roundForVersion` và logic "REJECT" đã có; `RoundLike` là kiểu tối thiểu, đã có test mẫu `sectionStatus.test.ts`.

**Giả định**: dùng đúng stack hiện có (React 19, Tailwind v4, zustand, motion, lucide); không thêm dependency; tổng hợp reject áp dụng cho scope đang chọn (`CouncilView` đã tính `scope`).

## 2. Thay đổi

### PR 1 - Layout + drawer

| File | Việc |
|---|---|
| `store/ui.ts` (sửa) | `layout: "conversation"\|"split"` + `setLayout`, key `plancouncil.layout`, `DEFAULT_LAYOUT = "conversation"` (readPref fallback). `drawerTab: "plan"\|"rejections"\|null`, `openDrawer(tab)`, `closeDrawer()`. `openDrawer("plan")` đặt `planFocus: null` (tránh effect mount của `PlanViewer` áp version cũ). `focusPlan` giữ chữ ký; thêm: nếu `layout==="conversation"` thì đồng thời `drawerTab:"plan"`. |
| `components/Dialog.tsx` (sửa) | Thêm prop `placement?: "center"\|"right"` (mặc định `center`, hành vi cũ không đổi). `right`: panel full-height, `w-[min(720px,100vw)]`, bỏ padding để PlanViewer chiếm toàn bộ. |
| `components/PlanDrawer.tsx` (mới) | `<Dialog placement="right" open={drawerTab!==null && tab==="council"}>` (cần `tab==="council"` vì `App` giữ các tab mounted, portal sẽ đè lên tab khác). Hai tab "Plan" (`<PlanViewer scope resolve/>`) và "Lý do reject (n)" (PR 2). Chỉ mount nội dung khi mở. Đóng khi `sectionFilter` chuyển sang khác null (bộ lọc tác dụng lên Timeline phía sau). |
| `components/CouncilView.tsx` (sửa) | `layout==="split"`: JSX cũ giữ nguyên. `conversation`: một cột `mx-auto max-w-4xl`, giữ thứ tự cũ, `MeetingRoom` bọc `max-w-2xl` (nó dùng `aspectRatio` nên tự phình theo chiều rộng, `MeetingRoom.tsx:94`), thêm nút "Xem plan · vN" (`openDrawer("plan")`), nút "Lý do reject" (PR 2); render `<PlanDrawer/>`. `useEffect` đóng drawer khi `id` phiên đổi. |
| `components/Timeline.tsx` (sửa) | Prop `canExpand` (mặc định true); `conversation` truyền false. Lý do: `Dialog` "Phóng to" + drawer là 2 Dialog chồng nhau, cả hai đăng ký Esc capture trên `document` và `stopPropagation` không chặn listener cùng node -> một lần Esc đóng cả hai. |
| `components/settings/InterfaceTab.tsx` (sửa) | Thêm nhóm "Bố cục họp" (Hội thoại / Chia đôi) bằng `RadioCards` sẵn có. |

Không cần `layoutFlags.ts`, `usePlanCopy`, `planSelection`, `useFocusTrap`, `PlanActionBar`, `ConversationLayout`.

### PR 2 - Tổng hợp reject

| File | Việc |
|---|---|
| `lib/rejections.ts` + `rejections.test.ts` (mới) | Hàm thuần `summarizeRejections(rounds, plans)` (mục 3) + test vitest. |
| `components/RejectionPanel.tsx` (mới) | Danh sách version (mới nhất trên cùng). Đầu panel: `x/y version bị reject`. Mỗi version là accordion: header `vN - bị reject ở vòng 1, 2` hoặc `đã hội tụ`; thân = từng round: REJECT hiện `summary_bubble`, `critical_issues`, `suggestions` (gập), chip section (text); APPROVE/ABSTAIN/lỗi hiện một dòng. Nút "Xem vN" = `focusPlan(n)` (chuyển sang tab Plan; badge section trong plan đã đánh dấu mục bị nêu). Panel đọc dữ liệu qua `useMemo(() => summarizeRejections(scope.rounds, scope.plans), [scope.rounds, scope.plans])` (tham chiếu không đổi khi `plan_delta`, vì draft nằm ở `SessionState.drafts`). |
| `components/PlanDrawer.tsx`, `CouncilView.tsx` (sửa) | Gắn tab/nút; badge = `rejectedVersionCount`. |

`RejectMarker` trong Timeline: **không làm** - `Timeline.tsx:468` đã có badge kết quả từng round và `ReviewMsg` đã mở rộng lý do.

## 3. Logic tổng hợp reject

```ts
export interface RejectItem { role: string; summary: string; issues: string[]; suggestions: string[]; sections: string[] }
export interface RoundSummary {
  round: number;
  outcome: "rejected" | "converged" | "open";      // open = chưa xong / ABSTAIN / lỗi, không REJECT
  counts: { APPROVE: number; REJECT: number; ABSTAIN: number; error: number; pending: number };
  rejects: RejectItem[];                            // review.verdict === "REJECT"
}
export interface VersionSummary { version: number; rounds: RoundSummary[]; everRejected: boolean }
export interface RejectionSummary { versions: VersionSummary[]; rejectedVersionCount: number; reviewedVersionCount: number }
export function summarizeRejections(rounds: readonly RoundView[], plans: readonly PlanView[]): RejectionSummary;
```

Quy tắc:
- `versions` lấy từ `plans` (giảm dần theo version); round gom theo `RoundView.planVersion`, giữ **mọi** round (không chỉ round cuối), sắp theo `round`.
- Slot -> đếm: `pending` -> `pending`; `review` -> `review.verdict`; `error` -> `error`; còn lại (chưa chạy) bỏ qua. `carried_over` vẫn là APPROVE, không cần cờ riêng.
- `outcome`: có REJECT -> `rejected`; `converged && completed` -> `converged`; còn lại -> `open`. Dùng trực tiếp `converged` do orchestrator tính (cùng cách `deriveSectionStatus` làm), không tự suy ra hội tụ.
- `everRejected` = có round `rejected`. `rejectedVersionCount` = số version `everRejected`; `reviewedVersionCount` = số version có >= 1 round.
- Predicate REJECT dùng chung hằng/hàm với `sectionStatus.ts:75` (xuất `isRejectReview`), tránh lệch logic.

## 4. Test

- `rejections.test.ts`: toàn APPROVE -> converged; REJECT rồi APPROVE cùng version -> giữ lý do vòng 1 (`everRejected`); ABSTAIN/error -> open; round đang chạy (pending, `completed:false`); version chưa có round; review thiếu `sections`; carried_over; đếm `rejectedVersionCount`.
- `ui.ts`: `openDrawer("plan")` xoá `planFocus`; `focusPlan` mở drawer chỉ khi `layout==="conversation"`; `layout` đọc đúng/fallback từ localStorage lỗi.
- Thủ công (live + History qua `openRecord`): "Xem vN" từ Timeline; mở drawer rồi đổi tab/phiên/scope; Final/Diff/Copy/Export trong drawer; Esc đóng drawer, không bật hộp dừng họp; lọc section từ drawer; chuyển `split` trong Cài đặt.
- Chạy `npm run verify` (hoặc `vitest run` + `tsc -b --noEmit`).

## 5. Rollback & Out of scope

**Rollback (feature flag)**: `layout` là tuỳ chọn người dùng, mặc định `DEFAULT_LAYOUT`. Người dùng gặp lỗi -> Cài đặt > Giao diện > "Chia đôi" (code cũ nguyên vẹn). Hotfix toàn bộ: đổi `DEFAULT_LAYOUT` về `"split"` (chỉ có tác dụng với người chưa tự chọn; muốn ép tất cả thì đổi key lưu thành `plancouncil.layout.v2`). Gỡ nhánh `split` sau 2 release ổn định. Không động tới dữ liệu cá nhân hay hạ tầng.

**Out of scope**: đổi IPC/orchestrator/models; cuộn tới heading (plan chưa có anchor); ma trận flag 3x3; top section; typing indicator, animation mới, popover MemberControls; RejectMarker; tóm tắt bằng LLM; thêm export/phím tắt; virtualize Timeline; i18n.
