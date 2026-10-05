// 텍스트 레이어가 없는 스캔 이미지 PDF(폰트 0개, pdftotext로도 텍스트 안 뽑히는 문서)에서
// macOS Vision(Live Text와 같은 엔진)으로 OCR해 텍스트 선택을 가능하게 한다.
// get_page_chars가 임베드 텍스트를 못 찾았을 때만 폴백으로 호출된다 — Windows/Linux는 나중에.
//
// 정밀한 글자별 좌표(VNRecognizedText.boundingBox(for:)) 대신, 인식된 한 줄(observation)의
// bounding box를 글자 수만큼 균등하게 나눠 쓴다. NSRange(UTF-16 오프셋) 계산을 안 해도 되고
// 별도 Vision 호출도 줄어든다 — 스캔 문서 특성상 어차피 완벽한 정밀도는 기대하기 어렵고,
// 드래그/더블클릭/단어 이동 전부 문자 배열 인덱스로 동작해서 위치가 몇 픽셀 어긋나도
// 클릭 위치 스냅(charIndexAt의 nearest 폴백)이 흡수한다.

use crate::TextChar;
use objc2::rc::Retained;
use objc2::AnyThread;
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString};
use objc2_vision::{VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel};

pub fn ocr_page_chars(png_bytes: &[u8], page_w_pt: f32, page_h_pt: f32) -> Result<Vec<TextChar>, String> {
    unsafe {
        let data = NSData::with_bytes(png_bytes);
        let options: Retained<NSDictionary<NSString>> = NSDictionary::new();
        let handler = VNImageRequestHandler::initWithData_options(
            VNImageRequestHandler::alloc(),
            &data,
            &options,
        );

        let request = VNRecognizeTextRequest::new();
        request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
        request.setUsesLanguageCorrection(true);
        let langs = NSArray::from_retained_slice(&[
            NSString::from_str("ko-KR"),
            NSString::from_str("en-US"),
        ]);
        request.setRecognitionLanguages(&langs);

        let req_super: Retained<VNRequest> = Retained::into_super(Retained::into_super(request.clone()));
        let requests: Retained<NSArray<VNRequest>> = NSArray::from_retained_slice(&[req_super]);
        handler
            .performRequests_error(&requests)
            .map_err(|e| e.to_string())?;

        let observations = request.results().ok_or("OCR: no results")?;

        // 화면상 읽는 순서로 재배열 — 단순히 y로만 정렬하면 표처럼 같은 행에 라벨 칸과
        // 내용 칸이 나란히 있는 레이아웃에서, 내용 칸이 여러 줄일 때 라벨이 그 줄들
        // 사이에 끼어들 수 있다(라벨이 세로로 가운데 정렬돼 있으면 특히). y구간이 겹치는
        // 것끼리 "같은 행"으로 묶고, 그 안에서만 x로 좌→우 정렬한 뒤 행 단위로 이어붙인다.
        let mut items: Vec<_> = observations.iter().map(|o| (o.boundingBox(), o)).collect();
        items.sort_by(|a, b| {
            let ay = a.0.origin.y;
            let by = b.0.origin.y;
            by.partial_cmp(&ay).unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut rows: Vec<(f64, f64, Vec<usize>)> = Vec::new(); // (y_min, y_max, item indices)
        for (idx, (bb, _)) in items.iter().enumerate() {
            let (y0, y1) = (bb.origin.y, bb.origin.y + bb.size.height);
            let merged = if let Some(row) = rows.last_mut() {
                let overlap = (y1.min(row.1) - y0.max(row.0)).max(0.0);
                let min_h = (y1 - y0).min(row.1 - row.0);
                if min_h > 0.0 && overlap > min_h * 0.3 {
                    row.0 = row.0.min(y0);
                    row.1 = row.1.max(y1);
                    row.2.push(idx);
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if !merged {
                rows.push((y0, y1, vec![idx]));
            }
        }
        // 행 내부를 그냥 x로만 정렬하면, 내용 칸이 여러 줄로 감싸져서 그 줄들이 전부 같은
        // 행에 묶인 경우(라벨 칸 옆에 문단이 있는 표) 각 줄의 왼쪽 시작 x가 OCR 바운딩박스
        // 오차로 미세하게 들쭉날쭉해서 문단 내 줄 순서(위→아래)가 뒤섞인다. 그래서 행을 묶을
        // 때와 같은 방식으로 x구간이 겹치는 항목끼리 "같은 칸"으로 먼저 묶고, 칸 단위로만
        // 좌→우 정렬한 뒤 칸 내부는 원래 순서(위→아래)를 그대로 유지한다.
        for row in rows.iter_mut() {
            let orig_order = row.2.clone();
            let mut by_x = orig_order.clone();
            by_x.sort_by(|&a, &b| items[a].0.origin.x.partial_cmp(&items[b].0.origin.x).unwrap_or(std::cmp::Ordering::Equal));
            let mut cols: Vec<(f64, f64, Vec<usize>)> = Vec::new(); // (x_min, x_max, item indices)
            for &i in &by_x {
                let bb = &items[i].0;
                let (x0, x1) = (bb.origin.x, bb.origin.x + bb.size.width);
                let merged = if let Some(col) = cols.last_mut() {
                    let overlap = (x1.min(col.1) - x0.max(col.0)).max(0.0);
                    let min_w = (x1 - x0).min(col.1 - col.0);
                    if min_w > 0.0 && overlap > min_w * 0.3 {
                        col.0 = col.0.min(x0);
                        col.1 = col.1.max(x1);
                        col.2.push(i);
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                if !merged {
                    cols.push((x0, x1, vec![i]));
                }
            }
            cols.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            row.2 = cols.into_iter()
                .flat_map(|c| orig_order.iter().copied().filter(|i| c.2.contains(i)).collect::<Vec<_>>())
                .collect();
        }
        let obs: Vec<_> = rows.into_iter().flat_map(|r| r.2).map(|i| items[i].1.clone()).collect();

        let mut chars = Vec::new();
        for ob in obs {
            let candidates = ob.topCandidates(1);
            let Some(top) = candidates.iter().next() else { continue };
            let text = top.string().to_string();
            let n = text.chars().count();
            if n == 0 {
                continue;
            }
            let bb = ob.boundingBox();
            let x0 = bb.origin.x as f32 * page_w_pt;
            let x1 = (bb.origin.x + bb.size.width) as f32 * page_w_pt;
            // Vision: origin.y는 박스 "아래쪽" 모서리(하단 원점 기준) — 위에서부터 재는
            // 우리 좌표계(y0=위쪽,y1=아래쪽)로 뒤집는다.
            let y1 = (1.0 - bb.origin.y) as f32 * page_h_pt;
            let y0 = (1.0 - bb.origin.y - bb.size.height) as f32 * page_h_pt;
            let start = chars.len();
            for (i, ch) in text.chars().enumerate() {
                let cx0 = x0 + (x1 - x0) * (i as f32 / n as f32);
                let cx1 = x0 + (x1 - x0) * ((i + 1) as f32 / n as f32);
                chars.push(TextChar { ch, rect: [cx0, y0, cx1, y1], font: None });
            }
            // extract_chars_with_font와 같은 이유로 줄 사이에 공백 한 칸을 끼워 인덱스가
            // 자연스럽게 이어지게 한다.
            if chars.len() > start {
                let last = &chars[chars.len() - 1];
                chars.push(TextChar { ch: ' ', rect: last.rect, font: None });
            }
        }
        Ok(chars)
    }
}
