import type {
  MediaOperationErrorCode,
  MediaOperationProgressMessageCode,
  MediaOperationReasonCode,
} from "../types/workflow";

export type Locale = "en" | "ko";

export type MediaOperationMessagesForLocale = {
  errorCodes: Record<MediaOperationErrorCode, string>;
  reasonCodes: Record<MediaOperationReasonCode, string>;
};

export type MessagesForLocale = {
  eyebrow: string;
  title: string;
  lede: string;
  toolReady: string;
  toolUnavailable: string;
  webPreviewMode: string;
  chooseMedia: string;
  chooseFolder: string;
  useSourceFolder: string;
  openFolder: string;
  sourceFile: string;
  outputFolder: string;
  sourceFolderLabel: string;
  customFolderLabel: string;
  startHere: string;
  pickSourceTitle: string;
  pickSourceBody: string;
  currentSettings: string;
  toolStatus: string;
  saveLocation: string;
  selection: string;
  sourceInfo: string;
  topCandidate: string;
  latestOutput: string;
  openOutputFolder: string;
  previewSelection: string;
  previewSelectionBody: string;
  fullFrameSelection: string;
  customSelection: (widthPercent: number, heightPercent: number) => string;
  resetSelection: string;
  previewUnavailable: string;
  previewHint: string;
  previewKeyboardHint: string;
  previewZoom: string;
  previewZoomFit: string;
  previewZoomActual: string;
  previewZoomOut: string;
  previewZoomIn: string;
  selectionRegionLabel: string;
  selectionHandleTopLeft: string;
  selectionHandleTopRight: string;
  selectionHandleBottomLeft: string;
  selectionHandleBottomRight: string;
  format: string;
  resolution: string;
  inputSize: string;
  frameRate: string;
  duration: string;
  contentScale: string;
  cropAspectRatio: string;
  cropAspectRatioFree: string;
  buildPreview: string;
  buildingPreview: string;
  runOptimizer: string;
  runningOptimizer: string;
  cancelOptimizer: string;
  mediaOperationQueued: string;
  mediaOperationInspecting: string;
  mediaOperationDecoding: string;
  mediaOperationEstimating: string;
  mediaOperationEncoding: string;
  mediaOperationFinalizing: string;
  nextStep: string;
  nextStepBody: string;
  guidance: string;
  optimizerHint: string;
  staticImageSourceTitle: string;
  staticImageSourceBody: string;
  pngAlreadySourceBody: string;
  convertToPng: string;
  convertingToPng: string;
  pngConversion: string;
  pngCreated: string;
  pngConversionFailed: string;
  savedTo: string;
  size: string;
  runtime: string;
  technicalDetails: string;
  command: string;
  advancedSettings: string;
  showAdvancedSettings: string;
  hideAdvancedSettings: string;
  hidePreviewCandidates: string;
  viewResults: string;
  hideResults: string;
  closePanel: string;
  advancedOptimizeFocus: string;
  optimizerGoal: string;
  optimizerGoalBalanced: string;
  optimizerGoalMotion: string;
  optimizerGoalQuality: string;
  qualityFrameDropInterval: string;
  frameDropDisabled: string;
  frameDropEvery: (interval: number) => string;
  advancedSearchDepth: string;
  optimizeFocusAuto: string;
  optimizeFocusQuality: string;
  optimizeFocusSize: string;
  searchDepthStandard: string;
  searchDepthThorough: string;
  advancedDetails: string;
  topPreviewCandidates: string;
  previewBudgetNote: (budget: number, shown: number) => string;
  noPlanYet: string;
  attemptLog: string;
  noAttemptsYet: string;
  warnings: string;
  searchSummary: string;
  bestOutput: string;
  sourceMatch: string;
  sourceMatchHint: string;
  selectionBasis: string;
  recommendedCandidate: string;
  selectedResult: string;
  selectionReasonBestWithinLimit: string;
  selectionReasonSmallestOversize: string;
  selectionReasonNoFitFound: string;
  inspectionFailed: string;
  mediaFoundationFallbackWarning: string;
  statusFirstFit: string;
  statusExhausted: string;
  statusNoOutput: string;
  statusPlanInvalid: string;
  statusInvokeFailed: string;
  skipped: string;
  fits: string;
  over: string;
  failed: string;
  desktopOnlyFeature: string;
  selectFramesFirst: string;
  selectSourceFirst: string;
  webPreviewNotice: string;
};

export const MESSAGES: Record<Locale, MessagesForLocale> = {
  en: {
    eyebrow: "StickerFit",
    title: "Discord sticker converter",
    lede: "Automatically converts media to match Discord's sticker format.",
    toolReady: "Ready",
    toolUnavailable: "Tool issue",
    webPreviewMode: "Web preview mode",
    chooseMedia: "Choose file",
    chooseFolder: "Choose folder",
    useSourceFolder: "Use source folder",
    openFolder: "Open folder",
    sourceFile: "Input file",
    outputFolder: "Output folder",
    sourceFolderLabel: "Source folder",
    customFolderLabel: "Custom folder",
    startHere: "Start here",
    pickSourceTitle: "Choose an input file",
    pickSourceBody: "StickerFit checks the file first, then optimizes it to fit Discord sticker limits.",
    currentSettings: "Current settings",
    toolStatus: "Tool status",
    saveLocation: "Save location",
    selection: "Selection",
    sourceInfo: "Source info",
    topCandidate: "Top preview candidate",
    latestOutput: "Latest output",
    openOutputFolder: "Open output folder",
    previewSelection: "Preview and selection",
    previewSelectionBody: "Drag on the source preview to choose the crop area. Drag inside the box to move it, or drag the corners to resize it.",
    fullFrameSelection: "Full frame",
    customSelection: (widthPercent, heightPercent) => `${widthPercent}% x ${heightPercent}%`,
    resetSelection: "Reset selection",
    previewUnavailable: "Preview is unavailable for this file.",
    previewHint: "",
    previewKeyboardHint: "",
    previewZoom: "Zoom",
    previewZoomFit: "Fit",
    previewZoomActual: "100%",
    previewZoomOut: "Zoom out",
    previewZoomIn: "Zoom in",
    selectionRegionLabel: "Selection region",
    selectionHandleTopLeft: "Resize selection from the top left",
    selectionHandleTopRight: "Resize selection from the top right",
    selectionHandleBottomLeft: "Resize selection from the bottom left",
    selectionHandleBottomRight: "Resize selection from the bottom right",
    format: "Format",
    resolution: "Resolution",
    inputSize: "Input size",
    frameRate: "Frame rate",
    duration: "Duration",
    contentScale: "Scale",
    cropAspectRatio: "Crop ratio",
    cropAspectRatioFree: "Free",
    buildPreview: "Preview candidates",
    buildingPreview: "Preparing preview...",
    runOptimizer: "Run optimizer",
    runningOptimizer: "Optimizing...",
    cancelOptimizer: "Cancel optimization",
    mediaOperationQueued: "Waiting to start...",
    mediaOperationInspecting: "Inspecting media...",
    mediaOperationDecoding: "Decoding media...",
    mediaOperationEstimating: "Estimating candidates...",
    mediaOperationEncoding: "Encoding output...",
    mediaOperationFinalizing: "Finalizing output...",
    nextStep: "Next step",
    nextStepBody: "Preview the candidate ladder first, then run the optimizer once the crop and frame selection look right.",
    guidance: "Guidance",
    optimizerHint: "Discord allows at most 5 seconds and 512 KiB.",
    staticImageSourceTitle: "Static image source",
    staticImageSourceBody: "This still image can be converted to PNG directly with the current crop applied.",
    pngAlreadySourceBody: "This source is already a PNG, so conversion is not required.",
    convertToPng: "Convert to PNG",
    convertingToPng: "Converting...",
    pngConversion: "PNG conversion",
    pngCreated: "PNG created",
    pngConversionFailed: "PNG conversion failed",
    savedTo: "Saved to",
    size: "Size",
    runtime: "Runtime",
    technicalDetails: "Technical details",
    command: "Command",
    advancedSettings: "Advanced settings",
    showAdvancedSettings: "Show advanced settings",
    hideAdvancedSettings: "Hide advanced settings",
    hidePreviewCandidates: "Hide preview candidates",
    viewResults: "View results",
    hideResults: "Hide results",
    closePanel: "Close panel",
    advancedOptimizeFocus: "Optimization focus",
    optimizerGoal: "Optimization goal",
    optimizerGoalBalanced: "Balanced",
    optimizerGoalMotion: "Motion first",
    optimizerGoalQuality: "Quality first",
    qualityFrameDropInterval: "Frame removal",
    frameDropDisabled: "Disabled",
    frameDropEvery: (interval) => `Remove every ${interval}th frame`,
    advancedSearchDepth: "Search depth",
    optimizeFocusAuto: "Auto",
    optimizeFocusQuality: "Quality first",
    optimizeFocusSize: "Smaller file first",
    searchDepthStandard: "Standard",
    searchDepthThorough: "Thorough",
    advancedDetails: "Advanced details",
    topPreviewCandidates: "Top preview candidates",
    previewBudgetNote: (budget, shown) => `Showing only the top ${shown} preview candidates out of ${budget} candidates.`,
    noPlanYet: "Run preview candidates to show the ranked candidates here.",
    attemptLog: "Attempt log",
    noAttemptsYet: "Optimizer attempts will appear here after you run the search.",
    warnings: "Warnings",
    searchSummary: "Result",
    bestOutput: "Limit status",
    sourceMatch: "Source match",
    sourceMatchHint: "Higher values mean the candidate stays closer to the source settings.",
    selectionBasis: "Selection basis",
    recommendedCandidate: "Recommended",
    selectedResult: "Selected result",
    selectionReasonBestWithinLimit:
      "Chose the closest-to-source candidate that still fits the Discord limit.",
    selectionReasonSmallestOversize:
      "No candidate fit the limit, so the smallest successful output is shown.",
    selectionReasonNoFitFound:
      "No successful output was produced, so there is no result to recommend.",
    inspectionFailed: "Inspection failed",
    mediaFoundationFallbackWarning:
      "Windows Media Foundation could not inspect this file, so StickerFit used the bundled FFmpeg decoder.",
    statusFirstFit: "Stopped at the first result that fit the Discord limit.",
    statusExhausted: "Checked all ranked candidates.",
    statusNoOutput: "No successful outputs were produced.",
    statusPlanInvalid: "The optimizer plan was invalid.",
    statusInvokeFailed: "The optimizer command failed before search could finish.",
    skipped: "Skipped",
    fits: "Fits limit",
    over: "Over limit",
    failed: "Failed",
    desktopOnlyFeature: "Available only in the desktop app.",
    selectFramesFirst: "Select at least one frame first.",
    selectSourceFirst: "Choose a source file first.",
    webPreviewNotice:
      "Web mode supports file inspection, crop, zoom, and timeline review. Optimization, export, and output folders remain desktop-only.",
  },
  ko: {
    eyebrow: "StickerFit",
    title: "디스코드용 스티커 컨버터",
    lede: "디스코드의 스티커 규격에 맞게 자동으로 변환해 줍니다.",
    toolReady: "준비 완료",
    toolUnavailable: "도구 문제",
    webPreviewMode: "웹 미리보기 모드",
    chooseMedia: "파일 선택",
    chooseFolder: "폴더 선택",
    useSourceFolder: "원본 폴더 사용",
    openFolder: "폴더 열기",
    sourceFile: "입력 파일",
    outputFolder: "출력 폴더",
    sourceFolderLabel: "원본 폴더",
    customFolderLabel: "사용자 폴더",
    startHere: "시작하기",
    pickSourceTitle: "입력 파일을 선택하세요",
    pickSourceBody: "StickerFit은 먼저 파일을 확인한 다음, 디스코드 스티커 제한에 맞게 최적화를 진행합니다.",
    currentSettings: "현재 설정",
    toolStatus: "도구 상태",
    saveLocation: "저장 위치",
    selection: "선택 영역",
    sourceInfo: "원본 정보",
    topCandidate: "상위 미리보기 후보",
    latestOutput: "최근 출력",
    openOutputFolder: "출력 폴더 열기",
    previewSelection: "미리보기와 선택 영역",
    previewSelectionBody: "원본 위에서 드래그해 영역을 선택하세요. 박스 안쪽을 드래그하면 이동하고, 모서리를 드래그하면 크기를 조절합니다.",
    fullFrameSelection: "전체 프레임",
    customSelection: (widthPercent, heightPercent) => `${widthPercent}% x ${heightPercent}%`,
    resetSelection: "선택 초기화",
    previewUnavailable: "이 파일은 미리보기를 표시할 수 없습니다.",
    previewHint: "",
    previewKeyboardHint: "",
    previewZoom: "확대 비율",
    previewZoomFit: "맞춤",
    previewZoomActual: "100%",
    previewZoomOut: "축소",
    previewZoomIn: "확대",
    selectionRegionLabel: "선택 영역",
    selectionHandleTopLeft: "좌상단에서 선택 영역 크기 조절",
    selectionHandleTopRight: "우상단에서 선택 영역 크기 조절",
    selectionHandleBottomLeft: "좌하단에서 선택 영역 크기 조절",
    selectionHandleBottomRight: "우하단에서 선택 영역 크기 조절",
    format: "포맷",
    resolution: "해상도",
    inputSize: "입력 크기",
    frameRate: "프레임레이트",
    duration: "길이",
    contentScale: "스케일",
    cropAspectRatio: "크롭 비율",
    cropAspectRatioFree: "자유",
    buildPreview: "후보 미리보기",
    buildingPreview: "후보를 준비하는 중...",
    runOptimizer: "최적화 실행",
    runningOptimizer: "최적화 중...",
    cancelOptimizer: "최적화 취소",
    mediaOperationQueued: "작업 시작을 기다리는 중...",
    mediaOperationInspecting: "미디어를 분석하는 중...",
    mediaOperationDecoding: "미디어를 디코딩하는 중...",
    mediaOperationEstimating: "후보를 계산하는 중...",
    mediaOperationEncoding: "출력을 인코딩하는 중...",
    mediaOperationFinalizing: "출력을 마무리하는 중...",
    nextStep: "다음 단계",
    nextStepBody: "먼저 후보 미리보기를 실행해 정렬된 래더를 확인한 다음, 크롭과 프레임 선택이 괜찮으면 최적화를 실행하세요.",
    guidance: "안내",
    optimizerHint: "디스코드는 최대 5초, 512 KiB까지 허용합니다.",
    staticImageSourceTitle: "정적 이미지 소스",
    staticImageSourceBody: "현재 크롭을 적용한 뒤 이 정적 이미지를 바로 PNG로 변환할 수 있습니다.",
    pngAlreadySourceBody: "이 소스는 이미 PNG이므로 변환이 필요하지 않습니다.",
    convertToPng: "PNG로 변환",
    convertingToPng: "변환 중...",
    pngConversion: "PNG 변환",
    pngCreated: "PNG 생성 완료",
    pngConversionFailed: "PNG 변환 실패",
    savedTo: "저장 위치",
    size: "크기",
    runtime: "실행 시간",
    technicalDetails: "기술 정보",
    command: "명령",
    advancedSettings: "고급 설정",
    showAdvancedSettings: "고급 설정 보기",
    hideAdvancedSettings: "고급 설정 숨기기",
    hidePreviewCandidates: "후보 미리보기 숨기기",
    viewResults: "결과 보기",
    hideResults: "결과 숨기기",
    closePanel: "패널 닫기",
    advancedOptimizeFocus: "최적화 초점",
    optimizerGoal: "최적화 목표",
    optimizerGoalBalanced: "균형",
    optimizerGoalMotion: "움직임 우선",
    optimizerGoalQuality: "화질 우선",
    qualityFrameDropInterval: "프레임 자동 제거",
    frameDropDisabled: "비활성화",
    frameDropEvery: (interval) => `${interval}번째마다 제거`,
    advancedSearchDepth: "탐색 강도",
    optimizeFocusAuto: "자동",
    optimizeFocusQuality: "품질 우선",
    optimizeFocusSize: "용량 우선",
    searchDepthStandard: "기본",
    searchDepthThorough: "꼼꼼하게",
    advancedDetails: "고급 정보",
    topPreviewCandidates: "상위 미리보기 후보",
    previewBudgetNote: (budget, shown) => `${budget}개 후보 중 상위 ${shown}개 미리보기 후보만 보여줍니다.`,
    noPlanYet: "후보 미리보기를 실행하면 이곳에 정렬된 후보들이 표시됩니다.",
    attemptLog: "시도 기록",
    noAttemptsYet: "최적화를 실행하면 이곳에 인코드 시도 결과가 표시됩니다.",
    warnings: "경고",
    searchSummary: "결과",
    bestOutput: "충족 여부",
    sourceMatch: "원본 보존도",
    sourceMatchHint: "값이 높을수록 원본 설정에 더 가깝습니다.",
    selectionBasis: "선택 기준",
    recommendedCandidate: "추천 후보",
    selectedResult: "선택된 결과",
    selectionReasonBestWithinLimit:
      "디스코드 제한 안에서 원본 특성에 가장 가까운 후보를 선택했습니다.",
    selectionReasonSmallestOversize:
      "제한을 만족한 후보가 없어, 성공한 출력 중 가장 작은 파일을 표시합니다.",
    selectionReasonNoFitFound:
      "성공적으로 생성된 출력이 없어 추천 결과를 표시할 수 없습니다.",
    inspectionFailed: "입력 검사 실패",
    mediaFoundationFallbackWarning:
      "Windows Media Foundation에서 이 파일을 검사하지 못해 StickerFit이 번들 FFmpeg 디코더를 사용했습니다.",
    statusFirstFit: "디스코드 제한을 만족한 첫 결과에서 중단했습니다.",
    statusExhausted: "정렬된 후보를 끝까지 확인했습니다.",
    statusNoOutput: "성공적으로 만들어진 출력이 없습니다.",
    statusPlanInvalid: "최적화 계획이 올바르지 않습니다.",
    statusInvokeFailed: "최적화 명령 실행에 실패했습니다.",
    skipped: "건너뜀",
    fits: "제한 충족",
    over: "제한 초과",
    failed: "실패",
    desktopOnlyFeature: "데스크톱 앱에서만 사용할 수 있습니다.",
    selectFramesFirst: "먼저 최소 한 개의 프레임을 선택하세요.",
    selectSourceFirst: "먼저 소스 파일을 선택하세요.",
    webPreviewNotice:
      "웹 모드에서는 파일 검사, 크롭, 확대, 타임라인 확인까지만 지원합니다. 최적화, 내보내기, 출력 폴더 기능은 데스크톱 앱 전용입니다.",
  },
};

export const MEDIA_OPERATION_PROGRESS_MESSAGES: Record<
  Locale,
  Record<MediaOperationProgressMessageCode, string>
> = {
  en: {
    "media-operation-queued": MESSAGES.en.mediaOperationQueued,
    "media-operation-inspecting": MESSAGES.en.mediaOperationInspecting,
    "media-operation-decoding": MESSAGES.en.mediaOperationDecoding,
    "media-operation-estimating": MESSAGES.en.mediaOperationEstimating,
    "media-operation-encoding": MESSAGES.en.mediaOperationEncoding,
    "media-operation-finalizing": MESSAGES.en.mediaOperationFinalizing,
  },
  ko: {
    "media-operation-queued": MESSAGES.ko.mediaOperationQueued,
    "media-operation-inspecting": MESSAGES.ko.mediaOperationInspecting,
    "media-operation-decoding": MESSAGES.ko.mediaOperationDecoding,
    "media-operation-estimating": MESSAGES.ko.mediaOperationEstimating,
    "media-operation-encoding": MESSAGES.ko.mediaOperationEncoding,
    "media-operation-finalizing": MESSAGES.ko.mediaOperationFinalizing,
  },
};

export const MEDIA_OPERATION_MESSAGES: Record<
  Locale,
  MediaOperationMessagesForLocale
> = {
  en: {
    errorCodes: {
      cancelled: "The operation was cancelled.",
      "timed-out": "The operation timed out.",
      "operation-conflict": "Another media operation is already in progress.",
      "invalid-request": "The media request is not valid.",
      "source-changed": "The source changed while the operation was running.",
      "media-input-too-large": "The source file is too large to process safely.",
      "media-dimensions-too-large": "The source dimensions are too large to process safely.",
      "media-frame-limit": "The source contains too many frames to process safely.",
      "decoded-byte-limit": "The decoded media would use too much memory.",
      "png-chunk-limit": "A PNG chunk is too large to process safely.",
      "malformed-media": "The source media could not be read.",
      "malformed-process-output": "The media tool returned an unreadable result.",
      "tool-missing": "A required media tool is unavailable.",
      "process-failed": "The media tool failed while processing the request.",
      "output-conflict": "The output path conflicts with an existing file.",
      "internal-task-failed": "The media operation failed unexpectedly.",
    },
    reasonCodes: {
      "no-frames-selected": "Select at least one frame before exporting.",
      "invalid-frame-selection": "The selected frames no longer match the current timeline.",
      "invalid-frame-duration": "One or more frame durations are invalid.",
      "duration-too-long": "The selected duration exceeds Discord's 5-second limit.",
      "invalid-crop": "The crop selection is invalid for the current source.",
      "invalid-output-directory": "Choose a valid output folder.",
      "unsupported-source-format": "This source format is not supported.",
      "unsupported-frame-preview": "A preview is not available for this source format.",
      "frame-preview-decode-failed": "The selected frame could not be decoded for preview.",
      "frame-preview-encode-failed": "The selected frame preview could not be encoded.",
      "decode-failed": "The source media could not be decoded.",
      "encode-failed": "The output could not be encoded.",
      "missing-output": "The media tool completed without creating an output file.",
      "plan-invalid": "The optimizer plan is not valid for the current input.",
      "invoke-failed": "The optimizer command could not be started.",
    },
  },
  ko: {
    errorCodes: {
      cancelled: "작업이 취소되었습니다.",
      "timed-out": "작업 시간이 초과되었습니다.",
      "operation-conflict": "다른 미디어 작업이 이미 진행 중입니다.",
      "invalid-request": "미디어 요청이 올바르지 않습니다.",
      "source-changed": "작업 중 원본이 변경되었습니다.",
      "media-input-too-large": "원본 파일이 너무 커서 안전하게 처리할 수 없습니다.",
      "media-dimensions-too-large": "원본 해상도가 너무 커서 안전하게 처리할 수 없습니다.",
      "media-frame-limit": "원본 프레임 수가 너무 많아 안전하게 처리할 수 없습니다.",
      "decoded-byte-limit": "디코딩한 미디어가 너무 많은 메모리를 사용합니다.",
      "png-chunk-limit": "PNG 청크 하나가 너무 커서 안전하게 처리할 수 없습니다.",
      "malformed-media": "원본 미디어를 읽을 수 없습니다.",
      "malformed-process-output": "미디어 도구가 읽을 수 없는 결과를 반환했습니다.",
      "tool-missing": "필수 미디어 도구를 사용할 수 없습니다.",
      "process-failed": "요청을 처리하는 동안 미디어 도구가 실패했습니다.",
      "output-conflict": "출력 경로가 기존 파일과 충돌합니다.",
      "internal-task-failed": "미디어 작업이 예기치 않게 실패했습니다.",
    },
    reasonCodes: {
      "no-frames-selected": "내보내기 전에 최소 한 개의 프레임을 선택하세요.",
      "invalid-frame-selection": "선택한 프레임이 현재 타임라인과 맞지 않습니다.",
      "invalid-frame-duration": "하나 이상의 프레임 길이가 올바르지 않습니다.",
      "duration-too-long": "선택한 길이가 디스코드의 5초 제한을 초과합니다.",
      "invalid-crop": "현재 원본에 적용할 수 없는 크롭 영역입니다.",
      "invalid-output-directory": "올바른 출력 폴더를 선택하세요.",
      "unsupported-source-format": "지원하지 않는 원본 형식입니다.",
      "unsupported-frame-preview": "이 원본 형식은 프레임 미리보기를 지원하지 않습니다.",
      "frame-preview-decode-failed": "선택한 프레임을 미리보기용으로 디코딩하지 못했습니다.",
      "frame-preview-encode-failed": "선택한 프레임 미리보기를 인코딩하지 못했습니다.",
      "decode-failed": "원본 미디어를 디코딩하지 못했습니다.",
      "encode-failed": "출력을 인코딩하지 못했습니다.",
      "missing-output": "미디어 도구가 출력 파일을 만들지 않았습니다.",
      "plan-invalid": "현재 입력에 사용할 수 없는 최적화 계획입니다.",
      "invoke-failed": "최적화 명령을 시작하지 못했습니다.",
    },
  },
};

export function mediaOperationMessage(
  locale: Locale,
  errorCode: MediaOperationErrorCode | null,
  reasonCode: MediaOperationReasonCode | null,
) {
  if (reasonCode) {
    return MEDIA_OPERATION_MESSAGES[locale].reasonCodes[reasonCode];
  }

  return errorCode
    ? MEDIA_OPERATION_MESSAGES[locale].errorCodes[errorCode]
    : null;
}

export function detectLocale(): Locale {
  if (typeof navigator !== "undefined" && navigator.language.toLowerCase().startsWith("ko")) {
    return "ko";
  }

  return "en";
}



