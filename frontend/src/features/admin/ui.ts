import { useUi } from '@/shared/i18n/ui'
const messages = {
    'Active flag': '有效标记',
    Yes: '是',
    No: '否',
    'Original content shown because this translation is unavailable.': '此语言暂无译文，显示原文。',
    Pages: '分页',
    'Check the title and both opening times.': '请检查标题及开放和关闭时间。',
    'This saves the changes as approved content.': '修改将保存为已审核通过的内容。',
    'Restore this account?': '恢复此账号？',
    Administration: '管理',
    'Back to map': '返回地图',
    'New place': '新增点位',
    'Content changes': '内容修改',
    'Photo submission': '图片提交',
    Review: '审核',
    'All places': '全部点位',
    Users: '用户',
    Markers: '点位',
    Edits: '文字提案',
    Images: '图片提案',
    'Access denied.': '此账号没有管理权限。',
    'Could not confirm access. Try again.': '无法确认权限，请重试。',
    Approve: '通过',
    Reject: '拒绝',
    Edit: '编辑',
    Cancel: '取消',
    Confirm: '确认',
    Refresh: '刷新',
    Previous: '上一页',
    Next: '下一页',
    'No items.': '暂无内容。',
    'Search users': '搜索用户',
    Disable: '停用',
    Restore: '恢复',
    'Reset password': '重置密码',
    Disabled: '已停用',
    Active: '正常',
    Admin: '管理员',
    User: '用户',
    'Saved, but the list could not refresh. Refresh before reviewing another item.':
        '已保存，但列表刷新失败。请刷新后再审核其他内容。',
    'Completed.': '操作已完成。',
    'The item changed. Review the refreshed list.': '内容已变化，请查看刷新后的列表。',
    'The outcome could not be confirmed. Refresh and inspect the item before trying again.':
        '无法确认操作结果，请刷新并检查条目后再决定是否重试。',
    'The request failed. Refresh and try again.': '请求失败，请刷新后重试。',
    'This hides the place from the map. All data is kept and can be restored.':
        '停用后地图将不再显示该点位，全部数据保留，可随时恢复。',
    'Restore this place with its previous visibility and review status?':
        '恢复此点位？原有公开范围和审核状态将保持不变。',
    'This disables the account and invalidates its sessions.': '这会停用账号并使其登录会话失效。',
    'This resets the password to the server-configured default and invalidates existing sessions.':
        '这会将密码重置为服务器配置的默认值，并使现有登录会话失效。',
    'Proposed changes': '提议的修改',
    'Current place': '当前点位',
    'Submitted by': '提交者',
    Visibility: '可见性',
    Language: '语言',
    Location: '位置',
    Submitted: '提交时间',
    'Edit approved content': '修改并通过内容',
    'Review status': '审核状态',
    'Image unavailable.': '图片暂不可用。',
    'Server update required': '等待服务器更新',
    'Checking access…': '正在确认权限…',
} as const
export function useAdminUi() {
    const ui = useUi()
    return {
        ...ui,
        message: (value: string) =>
            ui.language === 'zh' && Object.hasOwn(messages, value)
                ? messages[value as keyof typeof messages]
                : ui.message(value),
    }
}
